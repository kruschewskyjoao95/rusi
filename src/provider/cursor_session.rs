//! Sessão com o Cursor Agent CLI. O CLI não tem modo servidor: cada turno é um processo
//! `cursor-agent -p --output-format stream-json`, e a conversa continua com `--resume <chatId>`.
//! (O T3 usa o `@cursor/sdk` em TypeScript, que não tem equivalente em Rust.)

use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::config::AgentMode;
use crate::session::{AgentEvent, AgentSession, SessionConfig, SessionError};
use crate::tools::process::kill_process_group;

/// Argumentos de um turno. `-p` não consegue perguntar nada ao usuário: o que exigiria
/// aprovação é recusado, a menos que `--force` esteja ligado (modos `auto` e skip).
pub(super) fn build_args(config: &SessionConfig, chat_id: Option<&str>, prompt: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--stream-partial-output",
        // Sem terminal não há como confirmar a confiança no diretório.
        "--trust",
    ]
    .map(String::from)
    .to_vec();

    if config.mode == AgentMode::Plan && !config.dangerously_skip_permissions {
        args.push("--plan".into());
    } else if config.dangerously_skip_permissions || config.mode == AgentMode::Auto {
        args.push("--force".into());
    }
    if let Some(cwd) = &config.cwd {
        args.extend(["--workspace".into(), cwd.display().to_string()]);
    }
    if let Some(model) = &config.model {
        args.extend(["--model".into(), model.clone()]);
    }
    if let Some(id) = chat_id {
        args.extend(["--resume".into(), id.to_string()]);
    }
    args.push(prompt.to_string());
    args
}

#[derive(Debug, Default)]
pub(super) struct Translator {
    saw_partial: bool,
    final_text: Option<String>,
    session_announced: Option<String>,
}

/// Nome legível da ferramenta: a única chave de `tool_call`, sem o sufixo `ToolCall`.
fn tool_name(tool_call: &Value) -> Option<(String, &Value)> {
    let (key, body) = tool_call.as_object()?.iter().next()?;
    let name = key.strip_suffix("ToolCall").unwrap_or(key);
    Some((name.to_string(), body))
}

fn compact(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 2000 {
        format!("{}…", text.chars().take(2000).collect::<String>())
    } else {
        text
    }
}

impl Translator {
    pub(super) fn handle(&mut self, line: &str) -> Vec<AgentEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        match v["type"].as_str() {
            Some("system") if v["subtype"] == "init" => {
                let Some(id) = v["session_id"].as_str() else { return Vec::new() };
                if self.session_announced.as_deref() == Some(id) {
                    return Vec::new();
                }
                self.session_announced = Some(id.to_string());
                vec![AgentEvent::SessionStarted { session_id: id.to_string(), model: v["model"].as_str().map(str::to_string) }]
            }
            Some("assistant") => {
                let text: String = v["message"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect();
                if text.is_empty() {
                    return Vec::new();
                }
                // Com `--stream-partial-output`, os pedaços trazem `timestamp_ms` e a mensagem
                // final consolidada não; esta repete o texto já emitido.
                if v.get("timestamp_ms").is_some() {
                    self.saw_partial = true;
                    vec![AgentEvent::TextDelta(text)]
                } else if self.saw_partial {
                    self.final_text = Some(text);
                    Vec::new()
                } else {
                    self.final_text = Some(text.clone());
                    vec![AgentEvent::TextDelta(text)]
                }
            }
            Some("tool_call") => {
                let (Some(call_id), Some((name, body))) = (v["call_id"].as_str(), tool_name(&v["tool_call"])) else {
                    return Vec::new();
                };
                match v["subtype"].as_str() {
                    Some("started") => vec![AgentEvent::ToolUse {
                        id: call_id.to_string(),
                        name,
                        input: body["args"].clone(),
                    }],
                    Some("completed") => {
                        let result = &body["result"];
                        let failed = result.get("error").is_some() || result.get("rejected").is_some() || result.get("failure").is_some();
                        vec![AgentEvent::ToolResult { id: call_id.to_string(), output: compact(result), is_error: failed }]
                    }
                    _ => Vec::new(),
                }
            }
            Some("result") => {
                let is_error = v["is_error"].as_bool().unwrap_or(false) || v["subtype"].as_str().is_some_and(|s| s != "success");
                let text = v["result"].as_str().map(str::to_string).filter(|t| !t.is_empty()).or(self.final_text.take());
                self.saw_partial = false;
                self.final_text = None;
                vec![AgentEvent::TurnComplete {
                    text,
                    is_error,
                    usage: None,
                    cost_usd: None,
                    duration_ms: v["duration_ms"].as_u64(),
                    denied_tools: Vec::new(),
                }]
            }
            _ => Vec::new(),
        }
    }
}

pub struct CursorSession {
    binary: PathBuf,
    config: SessionConfig,
    chat_id: Arc<Mutex<Option<String>>>,
    running_pid: Arc<AtomicU32>,
    interrupted: Arc<AtomicBool>,
    tx: mpsc::UnboundedSender<AgentEvent>,
    events: mpsc::UnboundedReceiver<AgentEvent>,
}

impl CursorSession {
    pub(super) fn new(binary: &Path, config: &SessionConfig) -> Self {
        let (tx, events) = mpsc::unbounded_channel();
        Self {
            binary: binary.to_path_buf(),
            config: config.clone(),
            chat_id: Arc::new(Mutex::new(config.resume.clone())),
            running_pid: Arc::new(AtomicU32::new(0)),
            interrupted: Arc::new(AtomicBool::new(false)),
            tx,
            events,
        }
    }
}

#[async_trait]
impl AgentSession for CursorSession {
    async fn send(&mut self, prompt: &str) -> Result<(), SessionError> {
        if self.running_pid.load(Ordering::SeqCst) != 0 {
            return Err(SessionError::NotReady("já existe um turno em andamento".into()));
        }
        let chat_id = self.chat_id.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let mut cmd = Command::new(&self.binary);
        cmd.args(build_args(&self.config, chat_id.as_deref(), prompt))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        if let Some(cwd) = &self.config.cwd {
            cmd.current_dir(cwd);
        }
        let mut child = cmd.spawn().map_err(SessionError::Spawn)?;
        self.running_pid.store(child.id().unwrap_or(0), Ordering::SeqCst);
        self.interrupted.store(false, Ordering::SeqCst);

        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
        let tx = self.tx.clone();
        let chat = self.chat_id.clone();
        let running = self.running_pid.clone();
        let interrupted = self.interrupted.clone();

        tokio::spawn(async move {
            let stderr_task = tokio::spawn(async move {
                let mut text = String::new();
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if text.len() < 2000 {
                        text.push_str(&line);
                        text.push('\n');
                    }
                }
                text
            });

            let mut translator = Translator::default();
            let mut completed = false;
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                for event in translator.handle(&line) {
                    if let AgentEvent::SessionStarted { session_id, .. } = &event {
                        *chat.lock().unwrap_or_else(|p| p.into_inner()) = Some(session_id.clone());
                    }
                    if matches!(event, AgentEvent::TurnComplete { .. }) {
                        completed = true;
                        // O turno acabou para quem consome: já pode começar o próximo, mesmo
                        // que o processo ainda esteja encerrando.
                        running.store(0, Ordering::SeqCst);
                    }
                    let _ = tx.send(event);
                }
            }
            let status = child.wait().await.ok();
            let stderr_text = stderr_task.await.unwrap_or_default();

            if completed {
                return;
            }
            running.store(0, Ordering::SeqCst);
            if interrupted.load(Ordering::SeqCst) {
                let _ = tx.send(AgentEvent::TurnComplete {
                    text: None,
                    is_error: true,
                    usage: None,
                    cost_usd: None,
                    duration_ms: None,
                    denied_tools: Vec::new(),
                });
            } else {
                let detail = stderr_text.trim();
                let message = if detail.is_empty() {
                    format!("o cursor-agent terminou sem resultado (status {:?})", status.and_then(|s| s.code()))
                } else {
                    detail.to_string()
                };
                let _ = tx.send(AgentEvent::Error(message));
            }
        });
        Ok(())
    }

    async fn next_event(&mut self) -> Option<AgentEvent> {
        self.events.recv().await
    }

    async fn interrupt(&mut self) -> Result<(), SessionError> {
        let pid = self.running_pid.load(Ordering::SeqCst);
        if pid != 0 {
            self.interrupted.store(true, Ordering::SeqCst);
            kill_process_group(pid);
        }
        Ok(())
    }

    async fn close(&mut self) {
        let pid = self.running_pid.swap(0, Ordering::SeqCst);
        if pid != 0 {
            self.interrupted.store(true, Ordering::SeqCst);
            kill_process_group(pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg(mode: AgentMode, skip: bool) -> SessionConfig {
        SessionConfig { mode, dangerously_skip_permissions: skip, ..Default::default() }
    }

    #[test]
    fn test_args_by_mode_and_resume() {
        let plan = build_args(&cfg(AgentMode::Plan, false), None, "oi");
        assert!(plan.contains(&"--plan".to_string()) && !plan.contains(&"--force".to_string()));
        assert_eq!(plan.last().map(String::as_str), Some("oi"));

        let auto = build_args(&cfg(AgentMode::Auto, false), Some("chat-1"), "oi");
        assert!(auto.contains(&"--force".to_string()));
        let pos = auto.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(auto[pos + 1], "chat-1");

        let edits = build_args(&cfg(AgentMode::AcceptEdits, false), None, "oi");
        assert!(!edits.contains(&"--force".to_string()) && !edits.contains(&"--plan".to_string()));
        let skip = build_args(&cfg(AgentMode::Plan, true), None, "oi");
        assert!(skip.contains(&"--force".to_string()) && !skip.contains(&"--plan".to_string()));
    }

    #[test]
    fn test_init_is_announced_once() {
        let mut t = Translator::default();
        let line = json!({"type": "system", "subtype": "init", "session_id": "c1", "model": "m"}).to_string();
        assert_eq!(t.handle(&line), vec![AgentEvent::SessionStarted { session_id: "c1".into(), model: Some("m".into()) }]);
        assert!(t.handle(&line).is_empty());
    }

    #[test]
    fn test_partial_deltas_then_consolidated_message_is_not_repeated() {
        let mut t = Translator::default();
        let delta = |s: &str| json!({"type": "assistant", "timestamp_ms": 1, "message": {"content": [{"type": "text", "text": s}]}}).to_string();
        assert_eq!(t.handle(&delta("ola ")), vec![AgentEvent::TextDelta("ola ".into())]);
        assert_eq!(t.handle(&delta("mundo")), vec![AgentEvent::TextDelta("mundo".into())]);
        let full = json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "ola mundo"}]}}).to_string();
        assert!(t.handle(&full).is_empty());
        let done = t.handle(&json!({"type": "result", "subtype": "success", "is_error": false, "duration_ms": 5}).to_string());
        assert!(matches!(&done[0], AgentEvent::TurnComplete { text: Some(m), is_error: false, duration_ms: Some(5), .. } if m == "ola mundo"));
    }

    #[test]
    fn test_unpartial_message_is_emitted_as_delta() {
        let mut t = Translator::default();
        let full = json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "tudo"}]}}).to_string();
        assert_eq!(t.handle(&full), vec![AgentEvent::TextDelta("tudo".into())]);
    }

    #[test]
    fn test_tool_call_lifecycle() {
        let mut t = Translator::default();
        let started = json!({"type": "tool_call", "subtype": "started", "call_id": "k1", "tool_call": {"readToolCall": {"args": {"path": "a.rs"}}}}).to_string();
        assert_eq!(t.handle(&started), vec![AgentEvent::ToolUse { id: "k1".into(), name: "read".into(), input: json!({"path": "a.rs"}) }]);
        let ok = json!({"type": "tool_call", "subtype": "completed", "call_id": "k1", "tool_call": {"readToolCall": {"args": {}, "result": {"success": {"content": "x"}}}}}).to_string();
        assert!(matches!(&t.handle(&ok)[0], AgentEvent::ToolResult { is_error: false, .. }));
        let bad = json!({"type": "tool_call", "subtype": "completed", "call_id": "k2", "tool_call": {"shellToolCall": {"args": {}, "result": {"rejected": {"reason": "no"}}}}}).to_string();
        assert!(matches!(&t.handle(&bad)[0], AgentEvent::ToolResult { is_error: true, .. }));
    }

    #[test]
    fn test_error_result() {
        let mut t = Translator::default();
        let done = t.handle(&json!({"type": "result", "subtype": "error", "is_error": true, "result": "falhou"}).to_string());
        assert!(matches!(&done[0], AgentEvent::TurnComplete { is_error: true, text: Some(m), .. } if m == "falhou"));
        assert!(t.handle("lixo").is_empty());
    }
}
