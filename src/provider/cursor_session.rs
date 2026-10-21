//! Cursor Agent CLI: um processo `cursor-agent -p --output-format stream-json` por turno,
//! continuado com `--resume <chatId>` (ver `print_session`).
//! (O T3 usa o `@cursor/sdk` em TypeScript, que não tem equivalente em Rust.)

use serde_json::Value;

use super::print_session::{PrintSession, Translate};
use crate::config::AgentMode;
use crate::session::{AgentEvent, SessionConfig};

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

impl Translate for Translator {
    fn handle(&mut self, line: &str) -> Vec<AgentEvent> {
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

pub(super) fn session(binary: &std::path::Path, config: &SessionConfig) -> PrintSession {
    PrintSession::new("cursor-agent", binary, config, build_args, || Box::new(Translator::default()))
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
