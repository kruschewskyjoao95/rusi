//! Sessão com `pi --mode rpc`: JSONL em stdio. Pedidos `{"type": "...", "id": "..."}`;
//! respostas `{"type": "response", "id", "success"}`; todo o resto é evento da sessão.
//! O turno só termina em `agent_settled`.

use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex as AsyncMutex, mpsc};

use crate::config::{AgentMode, EffortLevel};
use crate::session::{AgentEvent, AgentSession, SessionConfig, SessionError, TokenUsage};
use crate::tools::process::kill_process_group;

const STDERR_TAIL_LINES: usize = 20;
const CLOSE_GRACE: Duration = Duration::from_secs(3);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// O Pi não tem aprovações: executa as ferramentas direto. Sem como pedir confirmação,
/// `plan` e `accept-edits` limitam o agente a ferramentas de leitura.
const READ_ONLY_TOOLS: &str = "read,grep,find,ls";

pub(super) fn build_args(config: &SessionConfig) -> Vec<String> {
    let mut args = vec!["--mode".to_string(), "rpc".to_string()];
    let unrestricted = config.dangerously_skip_permissions || config.mode == AgentMode::Auto;
    if !unrestricted {
        args.extend(["--tools".into(), READ_ONLY_TOOLS.into()]);
    }
    let thinking = match config.effort {
        EffortLevel::Low => "low",
        EffortLevel::Medium => "medium",
        EffortLevel::High => "high",
    };
    args.extend(["--thinking".into(), thinking.into()]);
    if let Some(model) = &config.model {
        args.extend(["--model".into(), model.clone()]);
    }
    if let Some(session) = &config.resume {
        args.extend(["--session".into(), session.clone()]);
    }
    args
}

/// Resposta a um `extension_ui_request` (diálogos de extensões do Pi).
pub(super) fn ui_reply(request: &Value, auto_accept: bool) -> Option<Value> {
    let id = request["id"].as_str()?;
    let reply = match request["method"].as_str()? {
        "confirm" => json!({ "type": "extension_ui_response", "id": id, "confirmed": auto_accept }),
        "select" | "input" | "editor" => json!({ "type": "extension_ui_response", "id": id, "cancelled": true }),
        // notify, setStatus, setWidget, setTitle… não esperam resposta.
        _ => return None,
    };
    Some(reply)
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

#[derive(Debug, Default)]
pub(super) struct Translator {
    last_text: Option<String>,
    error: Option<String>,
    usage: Option<TokenUsage>,
    cost: f64,
    saw_cost: bool,
}

impl Translator {
    pub(super) fn handle(&mut self, event: &Value) -> Vec<AgentEvent> {
        match event["type"].as_str() {
            Some("message_update") => {
                let delta = &event["assistantMessageEvent"];
                let text = delta["delta"].as_str().map(str::to_string);
                match (delta["type"].as_str(), text) {
                    (Some("text_delta"), Some(t)) => vec![AgentEvent::TextDelta(t)],
                    (Some("thinking_delta"), Some(t)) => vec![AgentEvent::ThinkingDelta(t)],
                    _ => Vec::new(),
                }
            }
            Some("message_end") if event["message"]["role"] == "assistant" => {
                self.on_assistant_message(&event["message"]);
                Vec::new()
            }
            Some("tool_execution_start") => {
                let (Some(id), name) = (event["toolCallId"].as_str(), event["toolName"].as_str().unwrap_or("tool")) else {
                    return Vec::new();
                };
                vec![AgentEvent::ToolUse { id: id.to_string(), name: name.to_string(), input: event["args"].clone() }]
            }
            Some("tool_execution_end") => event["toolCallId"]
                .as_str()
                .map(|id| AgentEvent::ToolResult {
                    id: id.to_string(),
                    output: content_text(&event["result"]["content"]),
                    is_error: event["isError"].as_bool().unwrap_or(false),
                })
                .into_iter()
                .collect(),
            Some("agent_settled") => vec![self.finish_turn(event["aborted"].as_bool().unwrap_or(false))],
            Some("response") if event["success"] == false => vec![AgentEvent::Error(format!(
                "o Pi recusou '{}': {}",
                event["command"].as_str().unwrap_or("comando"),
                event["error"].as_str().map(str::to_string).unwrap_or_else(|| event["error"].to_string())
            ))],
            _ => Vec::new(),
        }
    }

    fn on_assistant_message(&mut self, message: &Value) {
        let text = content_text(&message["content"]);
        if !text.is_empty() {
            self.last_text = Some(text);
        }
        if message["stopReason"] == "error" {
            self.error = Some(
                message["errorMessage"].as_str().unwrap_or("o Pi reportou um erro do modelo").to_string(),
            );
        }
        let usage = &message["usage"];
        if usage.is_object() {
            let n = |k: &str| usage[k].as_u64().unwrap_or(0);
            let total = self.usage.get_or_insert_with(TokenUsage::default);
            total.input_tokens += n("input");
            total.output_tokens += n("output");
            total.cache_read_tokens += n("cacheRead");
            total.cache_creation_tokens += n("cacheWrite");
            if let Some(c) = usage["cost"]["total"].as_f64() {
                self.cost += c;
                self.saw_cost = true;
            }
        }
    }

    fn finish_turn(&mut self, aborted: bool) -> AgentEvent {
        let error = self.error.take();
        let text = match (&error, self.last_text.take()) {
            (Some(e), _) => Some(e.clone()),
            (None, text) => text,
        };
        let cost = std::mem::take(&mut self.cost);
        AgentEvent::TurnComplete {
            text,
            is_error: aborted || error.is_some(),
            usage: self.usage.take(),
            cost_usd: std::mem::take(&mut self.saw_cost).then_some(cost),
            duration_ms: None,
            denied_tools: Vec::new(),
        }
    }
}

pub struct PiSession {
    child: Child,
    stdin: Arc<AsyncMutex<Option<ChildStdin>>>,
    events: mpsc::UnboundedReceiver<AgentEvent>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    next_id: u64,
}

fn tail_summary(tail: &Mutex<VecDeque<String>>) -> Option<String> {
    let tail = tail.lock().unwrap_or_else(|p| p.into_inner());
    (!tail.is_empty()).then(|| tail.iter().cloned().collect::<Vec<_>>().join("\n"))
}

async fn write_line(stdin: &AsyncMutex<Option<ChildStdin>>, value: &Value, tail: &Mutex<VecDeque<String>>) -> Result<(), SessionError> {
    let mut guard = stdin.lock().await;
    let Some(pipe) = guard.as_mut() else {
        return Err(SessionError::Closed(tail_summary(tail)));
    };
    let mut line = value.to_string();
    line.push('\n');
    let result = async {
        pipe.write_all(line.as_bytes()).await?;
        pipe.flush().await
    }
    .await;
    result.map_err(|_| SessionError::Closed(tail_summary(tail)))
}

impl PiSession {
    pub(super) async fn start(binary: &Path, config: &SessionConfig) -> Result<Self, SessionError> {
        let mut cmd = Command::new(binary);
        cmd.args(build_args(config))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }
        let mut child = cmd.spawn().map_err(SessionError::Spawn)?;
        let stdin = Arc::new(AsyncMutex::new(child.stdin.take()));
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let stderr_tail: Arc<Mutex<VecDeque<String>>> = Arc::default();
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

        // Linhas cruas do stdout; o handshake lê daqui antes de o tradutor assumir.
        let (raw_tx, mut raw_rx) = mpsc::unbounded_channel::<Value>();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(value) = serde_json::from_str::<Value>(line.trim_end_matches('\r')) {
                    let _ = raw_tx.send(value);
                }
            }
        });

        let mut session = Self { child, stdin, events: mpsc::unbounded_channel().1, stderr_tail, next_id: 1 };

        // Handshake: get_state devolve o arquivo da sessão (id durável para retomar).
        let state = match session.handshake(&mut raw_rx).await {
            Ok(state) => state,
            Err(err) => {
                session.close().await;
                return Err(err);
            }
        };
        let session_ref = state["sessionFile"]
            .as_str()
            .or(state["sessionId"].as_str())
            .map(str::to_string)
            .or_else(|| config.resume.clone());

        let (tx, events) = mpsc::unbounded_channel();
        if let Some(session_id) = session_ref {
            let _ = tx.send(AgentEvent::SessionStarted {
                session_id,
                model: state["model"]["id"].as_str().map(|id| match state["model"]["provider"].as_str() {
                    Some(p) => format!("{p}/{id}"),
                    None => id.to_string(),
                }),
            });
        }
        session.events = events;

        let auto_accept = config.dangerously_skip_permissions || config.mode == AgentMode::Auto;
        let ui_stdin = session.stdin.clone();
        let ui_tail = session.stderr_tail.clone();
        tokio::spawn(async move {
            let mut translator = Translator::default();
            while let Some(event) = raw_rx.recv().await {
                if event["type"] == "extension_ui_request"
                    && let Some(reply) = ui_reply(&event, auto_accept)
                {
                    let _ = write_line(&ui_stdin, &reply, &ui_tail).await;
                }
                for out in translator.handle(&event) {
                    if tx.send(out).is_err() {
                        return;
                    }
                }
            }
            let detail = tail_summary(&ui_tail).unwrap_or_else(|| "o processo do pi terminou inesperadamente".into());
            let _ = tx.send(AgentEvent::Error(detail));
        });

        Ok(session)
    }

    async fn handshake(&mut self, raw_rx: &mut mpsc::UnboundedReceiver<Value>) -> Result<Value, SessionError> {
        let id = self.take_id();
        write_line(&self.stdin, &json!({ "type": "get_state", "id": id }), &self.stderr_tail).await?;
        let wait = async {
            while let Some(value) = raw_rx.recv().await {
                if value["type"] == "response" && value["id"] == id.as_str() {
                    return Some(value);
                }
            }
            None
        };
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, wait).await {
            Ok(Some(resp)) if resp["success"] == true => Ok(resp["data"].clone()),
            Ok(Some(resp)) => Err(SessionError::NotReady(format!("o Pi recusou get_state: {}", resp["error"]))),
            Ok(None) => Err(SessionError::Closed(tail_summary(&self.stderr_tail))),
            Err(_) => Err(SessionError::NotReady("o Pi não respondeu ao handshake".into())),
        }
    }

    fn take_id(&mut self) -> String {
        let id = self.next_id;
        self.next_id += 1;
        id.to_string()
    }
}

#[async_trait]
impl AgentSession for PiSession {
    async fn send(&mut self, prompt: &str) -> Result<(), SessionError> {
        let id = self.take_id();
        write_line(&self.stdin, &json!({ "type": "prompt", "id": id, "message": prompt }), &self.stderr_tail).await
    }

    async fn next_event(&mut self) -> Option<AgentEvent> {
        self.events.recv().await
    }

    async fn interrupt(&mut self) -> Result<(), SessionError> {
        let id = self.take_id();
        write_line(&self.stdin, &json!({ "type": "abort", "id": id }), &self.stderr_tail).await
    }

    async fn close(&mut self) {
        *self.stdin.lock().await = None;
        if tokio::time::timeout(CLOSE_GRACE, self.child.wait()).await.is_err()
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
    fn test_args_restrict_tools_unless_auto() {
        let plan = build_args(&SessionConfig { mode: AgentMode::Plan, ..Default::default() }).join(" ");
        assert!(plan.contains("--tools read,grep,find,ls"));
        let edits = build_args(&SessionConfig { mode: AgentMode::AcceptEdits, ..Default::default() }).join(" ");
        assert!(edits.contains("--tools"));
        let auto = build_args(&SessionConfig { mode: AgentMode::Auto, ..Default::default() }).join(" ");
        assert!(!auto.contains("--tools"));
        let skip = build_args(&SessionConfig { dangerously_skip_permissions: true, ..Default::default() }).join(" ");
        assert!(!skip.contains("--tools"));
    }

    #[test]
    fn test_args_model_effort_and_resume() {
        let args = build_args(&SessionConfig {
            model: Some("anthropic/x".into()),
            effort: EffortLevel::High,
            resume: Some("/s/file.jsonl".into()),
            mode: AgentMode::Auto,
            ..Default::default()
        })
        .join(" ");
        assert!(args.starts_with("--mode rpc"));
        assert!(args.contains("--thinking high") && args.contains("--model anthropic/x") && args.contains("--session /s/file.jsonl"));
    }

    #[test]
    fn test_ui_replies() {
        assert_eq!(
            ui_reply(&json!({"id": "u1", "method": "confirm"}), false),
            Some(json!({"type": "extension_ui_response", "id": "u1", "confirmed": false}))
        );
        assert_eq!(
            ui_reply(&json!({"id": "u2", "method": "select"}), true),
            Some(json!({"type": "extension_ui_response", "id": "u2", "cancelled": true}))
        );
        assert_eq!(ui_reply(&json!({"id": "u3", "method": "notify"}), true), None);
    }

    #[test]
    fn test_streaming_and_tools() {
        let mut t = Translator::default();
        let text = t.handle(&json!({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "delta": "oi"}}));
        assert_eq!(text, vec![AgentEvent::TextDelta("oi".into())]);
        let think = t.handle(&json!({"type": "message_update", "assistantMessageEvent": {"type": "thinking_delta", "delta": "hm"}}));
        assert_eq!(think, vec![AgentEvent::ThinkingDelta("hm".into())]);
        assert!(t.handle(&json!({"type": "message_update", "assistantMessageEvent": {"type": "text_end"}})).is_empty());

        let started = t.handle(&json!({"type": "tool_execution_start", "toolCallId": "c1", "toolName": "bash", "args": {"command": "ls"}}));
        assert_eq!(started, vec![AgentEvent::ToolUse { id: "c1".into(), name: "bash".into(), input: json!({"command": "ls"}) }]);
        let ended = t.handle(&json!({"type": "tool_execution_end", "toolCallId": "c1", "isError": false, "result": {"content": [{"type": "text", "text": "a.rs"}]}}));
        assert_eq!(ended, vec![AgentEvent::ToolResult { id: "c1".into(), output: "a.rs".into(), is_error: false }]);
    }

    #[test]
    fn test_turn_settles_with_usage_and_cost() {
        let mut t = Translator::default();
        t.handle(&json!({"type": "message_end", "message": {
            "role": "assistant", "content": [{"type": "text", "text": "pong"}], "stopReason": "stop",
            "usage": {"input": 10, "output": 2, "cacheRead": 3, "cacheWrite": 1, "cost": {"total": 0.25}}}}));
        // Mensagens de outros papéis são ignoradas.
        t.handle(&json!({"type": "message_end", "message": {"role": "user", "content": "x"}}));
        let done = t.handle(&json!({"type": "agent_settled", "aborted": false}));
        assert_eq!(
            done,
            vec![AgentEvent::TurnComplete {
                text: Some("pong".into()),
                is_error: false,
                usage: Some(TokenUsage { input_tokens: 10, output_tokens: 2, cache_read_tokens: 3, cache_creation_tokens: 1 }),
                cost_usd: Some(0.25),
                duration_ms: None,
                denied_tools: vec![],
            }]
        );
        // Estado zerado no turno seguinte.
        let next = t.handle(&json!({"type": "agent_settled", "aborted": false}));
        assert!(matches!(&next[0], AgentEvent::TurnComplete { text: None, usage: None, cost_usd: None, .. }));
    }

    #[test]
    fn test_model_error_and_abort_are_errors() {
        let mut t = Translator::default();
        t.handle(&json!({"type": "message_end", "message": {"role": "assistant", "content": [], "stopReason": "error", "errorMessage": "sem créditos"}}));
        let done = t.handle(&json!({"type": "agent_settled", "aborted": false}));
        assert!(matches!(&done[0], AgentEvent::TurnComplete { is_error: true, text: Some(m), .. } if m == "sem créditos"));
        let aborted = t.handle(&json!({"type": "agent_settled", "aborted": true}));
        assert!(matches!(&aborted[0], AgentEvent::TurnComplete { is_error: true, .. }));
    }

    #[test]
    fn test_failed_response_becomes_error() {
        let mut t = Translator::default();
        let ev = t.handle(&json!({"type": "response", "command": "prompt", "success": false, "error": "ocupado"}));
        assert!(matches!(&ev[0], AgentEvent::Error(m) if m.contains("prompt") && m.contains("ocupado")));
        assert!(t.handle(&json!({"type": "response", "command": "prompt", "success": true})).is_empty());
    }
}
