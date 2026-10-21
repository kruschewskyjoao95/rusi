//! Antigravity CLI (`agy`): um processo `agy -p --output-format stream-json` por turno,
//! continuado com `--conversation <id>` (ver `print_session`).

use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

use super::print_session::{PrintSession, Translate};
use crate::config::{AgentMode, EffortLevel};
use crate::session::{AgentEvent, SessionConfig};

/// Argumentos de um turno. No modo `-p` o `agy` roda tudo sem perguntar (`always-proceed`),
/// então `accept-edits` usa `--sandbox` (cwd somente leitura para comandos) em vez de pedir aprovação.
pub(super) fn build_args(config: &SessionConfig, conversation: Option<&str>, prompt: &str) -> Vec<String> {
    let mut args: Vec<String> = ["--output-format", "stream-json"].map(String::from).to_vec();

    if config.dangerously_skip_permissions {
        args.push("--dangerously-skip-permissions".into());
    } else {
        match config.mode {
            AgentMode::Plan => args.extend(["--mode".into(), "plan".into()]),
            AgentMode::Auto => args.extend(["--mode".into(), "auto".into()]),
            _ => args.extend(["--mode".into(), "accept-edits".into(), "--sandbox".into()]),
        }
    }
    if let Some(model) = &config.model {
        args.extend(["--model".into(), model.clone()]);
    }
    // O padrão (médio) fica por conta do modelo: muitos ids já trazem o esforço no nome.
    match config.effort {
        EffortLevel::Low => args.extend(["--effort".into(), "low".into()]),
        EffortLevel::High => args.extend(["--effort".into(), "high".into()]),
        _ => {}
    }
    if let Some(id) = conversation {
        args.extend(["--conversation".into(), id.to_string()]);
    }
    // `-p` leva o prompt como valor; colado a ele, não vira outra flag.
    args.push(format!("-p={prompt}"));
    args
}

#[derive(Debug, Default)]
pub(super) struct Translator {
    /// Passos de ferramenta cujo `ToolUse` já foi emitido.
    started: HashSet<u64>,
}

fn tool_output_failed(output: &str) -> bool {
    output.strip_prefix("Command exited with code ").is_some_and(|rest| !rest.starts_with("0"))
}

impl Translate for Translator {
    fn handle(&mut self, line: &str) -> Vec<AgentEvent> {
        let Ok(value) = serde_json::from_str::<Value>(line) else { return Vec::new() };
        match value["event"].as_str() {
            Some("init") => vec![AgentEvent::SessionStarted {
                session_id: value["conversation_id"].as_str().unwrap_or_default().to_string(),
                model: value["init"]["model"].as_str().map(String::from),
            }],
            Some("step_update") => self.step(&value["step_update"]),
            Some("result") => {
                let result = &value["result"];
                let is_error = result["status"].as_str() != Some("SUCCESS");
                let text = result["response"].as_str().filter(|t| !t.is_empty()).or(result["error"].as_str()).unwrap_or_default();
                vec![AgentEvent::TurnComplete {
                    text: Some(text.to_string()),
                    is_error,
                    usage: None,
                    cost_usd: None,
                    duration_ms: result["duration_seconds"].as_f64().map(|s| (s * 1000.0) as u64),
                    denied_tools: Vec::new(),
                }]
            }
            _ => Vec::new(),
        }
    }
}

impl Translator {
    fn step(&mut self, step: &Value) -> Vec<AgentEvent> {
        match step["step_type"].as_str() {
            Some("agent_response") => match step["text_delta"].as_str().filter(|t| !t.is_empty()) {
                Some(text) => vec![AgentEvent::TextDelta(text.to_string())],
                None => Vec::new(),
            },
            Some("tool") => {
                let index = step["step_index"].as_u64().unwrap_or_default();
                let info = &step["tool_info"];
                let mut events = Vec::new();
                if self.started.insert(index) {
                    events.push(AgentEvent::ToolUse {
                        id: index.to_string(),
                        name: info["name"].as_str().or(step["tool_name"].as_str()).unwrap_or("tool").to_string(),
                        input: info["parameters"].clone(),
                    });
                }
                if step["state"].as_str() == Some("DONE") {
                    let output = info["output"].as_str().unwrap_or_default();
                    events.push(AgentEvent::ToolResult { id: index.to_string(), output: output.to_string(), is_error: tool_output_failed(output) });
                }
                events
            }
            _ => Vec::new(),
        }
    }
}

pub(super) fn session(binary: &Path, config: &SessionConfig) -> PrintSession {
    PrintSession::new("agy", binary, config, build_args, || Box::new(Translator::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(mode: AgentMode) -> SessionConfig {
        SessionConfig { mode, ..Default::default() }
    }

    #[test]
    fn test_build_args_maps_modes() {
        let accept = build_args(&config(AgentMode::AcceptEdits), None, "oi");
        assert!(accept.windows(2).any(|w| w == ["--mode", "accept-edits"]) && accept.contains(&"--sandbox".to_string()));
        assert_eq!(accept.last().unwrap(), "-p=oi");
        assert!(build_args(&config(AgentMode::Plan), None, "x").windows(2).any(|w| w == ["--mode", "plan"]));
        assert!(build_args(&config(AgentMode::Auto), None, "x").windows(2).any(|w| w == ["--mode", "auto"]));
        let skip = SessionConfig { dangerously_skip_permissions: true, ..Default::default() };
        let args = build_args(&skip, Some("c1"), "x");
        assert!(args.contains(&"--dangerously-skip-permissions".to_string()) && !args.contains(&"--mode".to_string()));
        assert!(args.windows(2).any(|w| w == ["--conversation", "c1"]));
    }

    #[test]
    fn test_translator_text_and_result() {
        let mut t = Translator::default();
        let init = t.handle(r#"{"event":"init","conversation_id":"c1","init":{"model":"m"}}"#);
        assert!(matches!(&init[..], [AgentEvent::SessionStarted { session_id, model: Some(m) }] if session_id == "c1" && m == "m"));
        let delta = t.handle(r#"{"event":"step_update","step_update":{"step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"olá"}}"#);
        assert!(matches!(&delta[..], [AgentEvent::TextDelta(s)] if s == "olá"));
        let done = t.handle(r#"{"event":"result","result":{"status":"SUCCESS","response":"olá","duration_seconds":1.5}}"#);
        assert!(matches!(&done[..], [AgentEvent::TurnComplete { is_error: false, duration_ms: Some(1500), .. }]));
        let quota = t.handle(r#"{"event":"result","result":{"status":"ERROR","response":"","error":"quota"}}"#);
        assert!(matches!(&quota[..], [AgentEvent::TurnComplete { is_error: true, text: Some(text), .. }] if text == "quota"));
    }

    #[test]
    fn test_translator_tool_steps() {
        let mut t = Translator::default();
        let active = t.handle(r#"{"event":"step_update","step_update":{"step_index":3,"state":"ACTIVE","step_type":"tool","tool_info":{"name":"run_command","parameters":{"CommandLine":"ls"}}}}"#);
        assert!(matches!(&active[..], [AgentEvent::ToolUse { id, name, .. }] if id == "3" && name == "run_command"));
        let done = t.handle(r#"{"event":"step_update","step_update":{"step_index":3,"state":"DONE","step_type":"tool","tool_info":{"name":"run_command","output":"Command exited with code 1.\n"}}}"#);
        assert!(matches!(&done[..], [AgentEvent::ToolResult { is_error: true, .. }]));
        // DONE sem ACTIVE antes emite os dois eventos.
        let both = t.handle(r#"{"event":"step_update","step_update":{"step_index":4,"state":"DONE","step_type":"tool","tool_info":{"name":"x","output":"Command exited with code 0."}}}"#);
        assert!(matches!(&both[..], [AgentEvent::ToolUse { .. }, AgentEvent::ToolResult { is_error: false, .. }]));
    }
}
