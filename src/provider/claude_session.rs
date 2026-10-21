//! Sessão com o CLI `claude` no modo `--print --input-format stream-json
//! --output-format stream-json`: um processo vivo por conversa, uma linha JSON por evento.

use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

use crate::config::{AgentMode, EffortLevel};
use crate::session::{AgentEvent, AgentSession, ApprovalRequest, SessionConfig, SessionError, TokenUsage};
use crate::tools::process::kill_process_group;

const STDERR_TAIL_LINES: usize = 20;
const CLOSE_GRACE: Duration = Duration::from_secs(3);

/// Argumentos do `claude` para uma sessão. Pedidos de permissão chegam ao Rusi como
/// `control_request` (`--permission-prompt-tool stdio`) e são respondidos por `respond_approval`.
pub(super) fn build_args(config: &SessionConfig) -> Vec<String> {
    let mut args: Vec<String> = [
        "--print",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-prompts",
        "host",
        "--permission-prompt-tool",
        "stdio",
    ]
    .map(String::from)
    .to_vec();

    let mode = if config.dangerously_skip_permissions {
        "bypassPermissions"
    } else {
        match config.mode {
            AgentMode::Plan => "plan",
            AgentMode::AcceptEdits => "acceptEdits",
            AgentMode::Auto => "auto",
        }
    };
    args.extend(["--permission-mode".into(), mode.into()]);

    let effort = match config.effort {
        EffortLevel::Low => "low",
        EffortLevel::Medium => "medium",
        EffortLevel::High => "high",
    };
    args.extend(["--effort".into(), effort.into()]);

    if let Some(model) = &config.model {
        args.extend(["--model".into(), model.clone()]);
    }
    if let Some(session_id) = &config.resume {
        args.extend(["--resume".into(), session_id.clone()]);
    }
    args
}

/// Traduz uma linha do stream do `claude` em zero ou mais eventos normalizados.
///
/// Com `--include-partial-messages`, texto e raciocínio chegam como deltas e a mensagem
/// `assistant` completa os repete; dela só se aproveitam os `tool_use`, que trazem o input inteiro.
pub(super) fn parse_line(line: &str) -> Vec<AgentEvent> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let nested = !v["parent_tool_use_id"].is_null();

    match v["type"].as_str() {
        Some("system") if v["subtype"] == "init" => v["session_id"]
            .as_str()
            .map(|id| AgentEvent::SessionStarted {
                session_id: id.to_string(),
                model: v["model"].as_str().map(str::to_string),
            })
            .into_iter()
            .collect(),

        Some("stream_event") if !nested => {
            let delta = &v["event"]["delta"];
            match (v["event"]["type"].as_str(), delta["type"].as_str()) {
                (Some("content_block_delta"), Some("text_delta")) => delta["text"]
                    .as_str()
                    .map(|t| AgentEvent::TextDelta(t.to_string()))
                    .into_iter()
                    .collect(),
                (Some("content_block_delta"), Some("thinking_delta")) => delta["thinking"]
                    .as_str()
                    .map(|t| AgentEvent::ThinkingDelta(t.to_string()))
                    .into_iter()
                    .collect(),
                _ => Vec::new(),
            }
        }

        Some("assistant") => v["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["type"] == "tool_use")
            .filter_map(|b| {
                Some(AgentEvent::ToolUse {
                    id: b["id"].as_str()?.to_string(),
                    name: b["name"].as_str()?.to_string(),
                    input: b["input"].clone(),
                })
            })
            .collect(),

        Some("user") => v["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["type"] == "tool_result")
            .filter_map(|b| {
                Some(AgentEvent::ToolResult {
                    id: b["tool_use_id"].as_str()?.to_string(),
                    output: flatten_tool_output(&b["content"]),
                    is_error: b["is_error"].as_bool().unwrap_or(false),
                })
            })
            .collect(),

        Some("control_request") if v["request"]["subtype"] == "can_use_tool" => v["request_id"]
            .as_str()
            .zip(v["request"]["tool_name"].as_str())
            .map(|(id, tool)| {
                AgentEvent::ApprovalRequested(ApprovalRequest {
                    id: id.to_string(),
                    tool: tool.to_string(),
                    input: v["request"]["input"].clone(),
                })
            })
            .into_iter()
            .collect(),

        Some("result") => vec![AgentEvent::TurnComplete {
            text: v["result"].as_str().map(str::to_string),
            is_error: v["is_error"].as_bool().unwrap_or(false) || v["subtype"].as_str().is_some_and(|s| s != "success"),
            usage: parse_usage(&v["usage"]),
            cost_usd: v["total_cost_usd"].as_f64(),
            duration_ms: v["duration_ms"].as_u64(),
            denied_tools: v["permission_denials"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|d| d["tool_name"].as_str().map(str::to_string))
                .collect(),
        }],

        _ => Vec::new(),
    }
}

fn flatten_tool_output(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn parse_usage(usage: &Value) -> Option<TokenUsage> {
    if !usage.is_object() {
        return None;
    }
    let n = |key: &str| usage[key].as_u64().unwrap_or(0);
    Some(TokenUsage {
        input_tokens: n("input_tokens"),
        output_tokens: n("output_tokens"),
        cache_read_tokens: n("cache_read_input_tokens"),
        cache_creation_tokens: n("cache_creation_input_tokens"),
    })
}

/// Resposta a um `can_use_tool`: `allow` devolve o input (o agente pode usá-lo editado).
fn approval_response(request_id: &str, approved: bool, input: Value) -> Value {
    let decision = if approved {
        json!({ "behavior": "allow", "updatedInput": input })
    } else {
        json!({ "behavior": "deny", "message": "Negado pelo usuário no Rusi" })
    };
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": decision },
    })
}

pub struct ClaudeSession {
    child: Child,
    stdin: Option<ChildStdin>,
    events: mpsc::UnboundedReceiver<AgentEvent>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    /// Input de cada pedido aberto: aprovar exige devolvê-lo em `updatedInput`.
    pending_approvals: HashMap<String, Value>,
}

impl ClaudeSession {
    pub(super) fn spawn(binary: &Path, config: &SessionConfig) -> Result<Self, SessionError> {
        let mut cmd = Command::new(binary);
        cmd.args(build_args(config))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            // Grupo próprio: encerrar a sessão derruba também os comandos que o agente lançou.
            .process_group(0);
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }

        let mut child = cmd.spawn().map_err(SessionError::Spawn)?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let (tx, events) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                for event in parse_line(&line) {
                    if tx.send(event).is_err() {
                        return;
                    }
                }
            }
        });

        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut tail = tail.lock().unwrap_or_else(|p| p.into_inner());
                if tail.len() == STDERR_TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        });

        Ok(Self { child, stdin, events, stderr_tail, pending_approvals: HashMap::new() })
    }

    fn stderr_summary(&self) -> Option<String> {
        let tail = self.stderr_tail.lock().unwrap_or_else(|p| p.into_inner());
        (!tail.is_empty()).then(|| tail.iter().cloned().collect::<Vec<_>>().join("\n"))
    }

    async fn write_line(&mut self, value: Value) -> Result<(), SessionError> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(SessionError::Closed(self.stderr_summary()));
        };
        let mut line = value.to_string();
        line.push('\n');
        let result = async {
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await
        }
        .await;
        result.map_err(|_| SessionError::Closed(self.stderr_summary()))
    }
}

#[async_trait]
impl AgentSession for ClaudeSession {
    async fn send(&mut self, prompt: &str) -> Result<(), SessionError> {
        self.write_line(json!({
            "type": "user",
            "message": { "role": "user", "content": prompt },
        }))
        .await
    }

    async fn next_event(&mut self) -> Option<AgentEvent> {
        match self.events.recv().await {
            Some(event) => {
                if let AgentEvent::ApprovalRequested(request) = &event {
                    self.pending_approvals.insert(request.id.clone(), request.input.clone());
                }
                Some(event)
            }
            None => {
                // stdout fechou: o processo terminou sem fechar o turno.
                let status = self.child.wait().await.ok();
                let ok = status.is_some_and(|s| s.success());
                match (ok, self.stderr_summary()) {
                    (true, _) => None,
                    (false, detail) => Some(AgentEvent::Error(
                        detail.unwrap_or_else(|| "o processo do claude terminou inesperadamente".into()),
                    )),
                }
                .inspect(|_| self.stdin = None)
            }
        }
    }

    async fn respond_approval(&mut self, id: &str, approved: bool) -> Result<(), SessionError> {
        let input = self.pending_approvals.remove(id).unwrap_or(Value::Null);
        self.write_line(approval_response(id, approved, input)).await
    }

    async fn interrupt(&mut self) -> Result<(), SessionError> {
        self.write_line(json!({
            "type": "control_request",
            "request_id": uuid::Uuid::new_v4().to_string(),
            "request": { "subtype": "interrupt" },
        }))
        .await
    }

    async fn close(&mut self) {
        // EOF no stdin é o encerramento limpo; se não bastar, derruba o grupo inteiro.
        self.stdin = None;
        let exited = tokio::time::timeout(CLOSE_GRACE, self.child.wait()).await;
        if exited.is_err()
            && let Some(pid) = self.child.id()
        {
            kill_process_group(pid);
            let _ = self.child.wait().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_map_mode_effort_model_resume() {
        let args = build_args(&SessionConfig {
            mode: AgentMode::Plan,
            effort: EffortLevel::High,
            model: Some("opus".into()),
            resume: Some("sess-1".into()),
            ..Default::default()
        });
        let joined = args.join(" ");
        assert!(joined.contains("--permission-mode plan"));
        assert!(joined.contains("--effort high"));
        assert!(joined.contains("--model opus"));
        assert!(joined.contains("--resume sess-1"));
        assert!(joined.contains("--input-format stream-json --output-format stream-json"));
    }

    #[test]
    fn test_skip_permissions_overrides_mode() {
        let args = build_args(&SessionConfig {
            mode: AgentMode::Plan,
            dangerously_skip_permissions: true,
            ..Default::default()
        });
        assert!(args.join(" ").contains("--permission-mode bypassPermissions"));
    }

    #[test]
    fn test_parse_init() {
        let ev = parse_line(r#"{"type":"system","subtype":"init","session_id":"s1","model":"m"}"#);
        assert_eq!(
            ev,
            vec![AgentEvent::SessionStarted { session_id: "s1".into(), model: Some("m".into()) }]
        );
        assert!(parse_line(r#"{"type":"system","subtype":"status","session_id":"s1"}"#).is_empty());
    }

    #[test]
    fn test_parse_text_and_thinking_deltas() {
        let text = r#"{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"oi"}}}"#;
        assert_eq!(parse_line(text), vec![AgentEvent::TextDelta("oi".into())]);
        let think = r#"{"type":"stream_event","parent_tool_use_id":null,"event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hm"}}}"#;
        assert_eq!(parse_line(think), vec![AgentEvent::ThinkingDelta("hm".into())]);
        let input = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{"}}}"#;
        assert!(parse_line(input).is_empty());
    }

    #[test]
    fn test_nested_subagent_deltas_are_ignored() {
        let nested = r#"{"type":"stream_event","parent_tool_use_id":"toolu_1","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"x"}}}"#;
        assert!(parse_line(nested).is_empty());
    }

    #[test]
    fn test_assistant_message_only_yields_tool_use() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"vou ler"},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/a"}}]}}"#;
        assert_eq!(
            parse_line(line),
            vec![AgentEvent::ToolUse { id: "t1".into(), name: "Read".into(), input: json!({"file_path": "/a"}) }]
        );
    }

    #[test]
    fn test_tool_result_string_and_blocks() {
        let s = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#;
        assert_eq!(
            parse_line(s),
            vec![AgentEvent::ToolResult { id: "t1".into(), output: "ok".into(), is_error: false }]
        );
        let b = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","is_error":true,"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}]}}"#;
        assert_eq!(
            parse_line(b),
            vec![AgentEvent::ToolResult { id: "t2".into(), output: "a\nb".into(), is_error: true }]
        );
    }

    #[test]
    fn test_parse_result() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","duration_ms":10,"total_cost_usd":0.5,"usage":{"input_tokens":2,"output_tokens":4,"cache_read_input_tokens":8,"cache_creation_input_tokens":16},"permission_denials":[{"tool_name":"Bash"}]}"#;
        assert_eq!(
            parse_line(line),
            vec![AgentEvent::TurnComplete {
                text: Some("ok".into()),
                is_error: false,
                usage: Some(TokenUsage { input_tokens: 2, output_tokens: 4, cache_read_tokens: 8, cache_creation_tokens: 16 }),
                cost_usd: Some(0.5),
                duration_ms: Some(10),
                denied_tools: vec!["Bash".into()],
            }]
        );
    }

    #[test]
    fn test_error_subtype_marks_turn_as_error() {
        let ev = parse_line(r#"{"type":"result","subtype":"error_during_execution","is_error":false}"#);
        assert!(matches!(&ev[0], AgentEvent::TurnComplete { is_error: true, .. }));
    }

    #[test]
    fn test_garbage_lines_are_ignored() {
        assert!(parse_line("not json").is_empty());
        assert!(parse_line("{}").is_empty());
    }
}
