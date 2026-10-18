//! Sessão com `codex app-server` (JSON-RPC sobre stdio): `initialize`, `thread/start|resume`,
//! `turn/start`, `turn/interrupt`, notificações `item/*` e `turn/completed`, e pedidos de
//! aprovação do servidor.

use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::config::{AgentMode, EffortLevel};
use crate::rpc::{Incoming, RpcError, RpcPeer};
use crate::session::{AgentEvent, AgentSession, SessionConfig, SessionError, TokenUsage};

/// Política traduzida do modo do Rusi para o vocabulário do Codex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CodexPolicy {
    pub approval_policy: &'static str,
    pub sandbox: &'static str,
    /// Responde `accept` aos pedidos de aprovação (senão `decline`).
    pub auto_accept: bool,
}

pub(super) fn policy_for(config: &SessionConfig) -> CodexPolicy {
    if config.dangerously_skip_permissions {
        return CodexPolicy { approval_policy: "never", sandbox: "danger-full-access", auto_accept: true };
    }
    match config.mode {
        AgentMode::Plan => CodexPolicy { approval_policy: "never", sandbox: "read-only", auto_accept: false },
        AgentMode::AcceptEdits => {
            CodexPolicy { approval_policy: "on-request", sandbox: "workspace-write", auto_accept: false }
        }
        AgentMode::Auto => CodexPolicy { approval_policy: "on-request", sandbox: "workspace-write", auto_accept: true },
    }
}

fn effort_name(effort: EffortLevel) -> &'static str {
    match effort {
        EffortLevel::Low => "low",
        EffortLevel::Medium => "medium",
        EffortLevel::High => "high",
    }
}

/// Estado acumulado para montar o `TurnComplete` a partir das notificações do turno.
#[derive(Debug, Default)]
pub(super) struct Translator {
    last_agent_text: Option<String>,
    last_error: Option<String>,
    usage: Option<TokenUsage>,
    denied: Vec<String>,
}

impl Translator {
    pub(super) fn note_denied(&mut self, tool: &str) {
        self.denied.push(tool.to_string());
    }

    /// Traduz uma notificação do app-server em eventos normalizados.
    pub(super) fn handle(&mut self, method: &str, params: &Value) -> Vec<AgentEvent> {
        match method {
            "item/agentMessage/delta" => string_event(params, "delta", AgentEvent::TextDelta),
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                string_event(params, "delta", AgentEvent::ThinkingDelta)
            }
            "item/started" => item_started(&params["item"]).into_iter().collect(),
            "item/completed" => {
                let item = &params["item"];
                if item["type"] == "agentMessage" {
                    self.last_agent_text = item["text"].as_str().map(str::to_string);
                }
                item_completed(item).into_iter().collect()
            }
            "thread/tokenUsage/updated" => {
                self.add_usage(&params["tokenUsage"]["last"]);
                Vec::new()
            }
            "error" => {
                if let Some(message) = params["error"]["message"].as_str() {
                    self.last_error = Some(message.to_string());
                }
                Vec::new()
            }
            "turn/completed" => vec![self.finish_turn(&params["turn"])],
            _ => Vec::new(),
        }
    }

    fn add_usage(&mut self, last: &Value) {
        if !last.is_object() {
            return;
        }
        let n = |key: &str| last[key].as_u64().unwrap_or(0);
        let usage = self.usage.get_or_insert_with(TokenUsage::default);
        usage.input_tokens += n("inputTokens");
        usage.output_tokens += n("outputTokens");
        usage.cache_read_tokens += n("cachedInputTokens");
        usage.cache_creation_tokens += n("cacheWriteInputTokens");
    }

    fn finish_turn(&mut self, turn: &Value) -> AgentEvent {
        let status = turn["status"].as_str().unwrap_or("completed");
        let error = turn["error"]["message"].as_str().map(str::to_string).or(self.last_error.take());
        let text = match (&self.last_agent_text, &error) {
            (Some(t), _) => Some(t.clone()),
            (None, Some(e)) => Some(e.clone()),
            (None, None) => None,
        };
        let event = AgentEvent::TurnComplete {
            text,
            is_error: status != "completed",
            usage: self.usage.take(),
            cost_usd: None,
            duration_ms: turn["durationMs"].as_u64(),
            denied_tools: std::mem::take(&mut self.denied),
        };
        self.last_agent_text = None;
        event
    }
}

fn string_event(params: &Value, key: &str, make: fn(String) -> AgentEvent) -> Vec<AgentEvent> {
    params[key].as_str().map(|s| make(s.to_string())).into_iter().collect()
}

fn item_started(item: &Value) -> Option<AgentEvent> {
    let id = item["id"].as_str()?.to_string();
    let (name, input) = match item["type"].as_str()? {
        "commandExecution" => ("command".to_string(), json!({ "command": item["command"], "cwd": item["cwd"] })),
        "fileChange" => (
            "file_change".to_string(),
            json!({
                "changes": item["changes"].as_array().into_iter().flatten()
                    .map(|c| json!({ "path": c["path"], "kind": c["kind"] }))
                    .collect::<Vec<_>>()
            }),
        ),
        "mcpToolCall" => (
            format!("{}/{}", item["server"].as_str().unwrap_or("mcp"), item["tool"].as_str().unwrap_or("?")),
            item["arguments"].clone(),
        ),
        "dynamicToolCall" => (item["tool"].as_str().unwrap_or("tool").to_string(), item["arguments"].clone()),
        "webSearch" => ("web_search".to_string(), json!({ "query": item["query"] })),
        _ => return None,
    };
    Some(AgentEvent::ToolUse { id, name, input })
}

fn item_completed(item: &Value) -> Option<AgentEvent> {
    let id = item["id"].as_str()?.to_string();
    let failed = |status: &str| matches!(status, "failed" | "declined");
    let (output, is_error) = match item["type"].as_str()? {
        "commandExecution" => (
            item["aggregatedOutput"].as_str().unwrap_or("").to_string(),
            failed(item["status"].as_str().unwrap_or("")) || item["exitCode"].as_i64().is_some_and(|c| c != 0),
        ),
        "fileChange" => (
            format!("{} arquivo(s)", item["changes"].as_array().map_or(0, Vec::len)),
            failed(item["status"].as_str().unwrap_or("")),
        ),
        "mcpToolCall" => {
            let err = item["error"]["message"].as_str();
            let output = err.map(str::to_string).unwrap_or_else(|| {
                if item["result"].is_null() { String::new() } else { item["result"].to_string() }
            });
            (output, err.is_some() || failed(item["status"].as_str().unwrap_or("")))
        }
        "dynamicToolCall" => (String::new(), item["success"].as_bool() == Some(false)),
        "webSearch" => (String::new(), false),
        _ => return None,
    };
    Some(AgentEvent::ToolResult { id, output, is_error })
}

/// Resposta a um pedido do servidor. `Err` vira erro JSON-RPC; `denied` nomeia a
/// ferramenta negada, para o relatório do turno.
#[derive(Debug, PartialEq)]
pub(super) struct ServerReply {
    pub result: Result<Value, (i64, &'static str)>,
    pub denied: Option<&'static str>,
}

pub(super) fn answer_server_request(method: &str, params: &Value, auto_accept: bool) -> ServerReply {
    let decision = |accepted: bool, denied_name: &'static str| ServerReply {
        result: Ok(json!({ "decision": if accepted { "accept" } else { "decline" } })),
        denied: (!accepted).then_some(denied_name),
    };
    match method {
        "item/commandExecution/requestApproval" => decision(auto_accept, "command"),
        "item/fileChange/requestApproval" => decision(auto_accept, "file_change"),
        "execCommandApproval" | "applyPatchApproval" => ServerReply {
            result: Ok(json!({ "decision": if auto_accept { "approved" } else { "denied" } })),
            denied: (!auto_accept).then_some("command"),
        },
        "item/permissions/requestApproval" => ServerReply {
            result: Ok(json!({
                "permissions": if auto_accept { params["permissions"].clone() } else { json!({}) },
                "scope": "turn",
            })),
            denied: (!auto_accept).then_some("permissions"),
        },
        "item/tool/requestUserInput" => ServerReply { result: Ok(json!({ "answers": {} })), denied: None },
        "mcpServer/elicitation/request" => ServerReply { result: Ok(json!({ "action": "decline" })), denied: None },
        "item/tool/call" => {
            ServerReply { result: Ok(json!({ "contentItems": [], "success": false })), denied: None }
        }
        "account/chatgptAuthTokens/refresh" | "attestation/generate" => {
            ServerReply { result: Err((-32001, "não suportado pelo Rusi")), denied: None }
        }
        _ => ServerReply { result: Err((-32601, "método não suportado")), denied: None },
    }
}

pub struct CodexSession {
    peer: Arc<RpcPeer>,
    thread_id: String,
    turn_id: Arc<Mutex<Option<String>>>,
    events: mpsc::UnboundedReceiver<AgentEvent>,
    effort: EffortLevel,
}

fn rpc_to_session(err: RpcError) -> SessionError {
    match err {
        RpcError::Closed(detail) => SessionError::Closed(detail),
        RpcError::Remote { message, .. } => SessionError::NotReady(message),
    }
}

impl CodexSession {
    pub(super) async fn start(binary: &Path, config: &SessionConfig) -> Result<Self, SessionError> {
        let mut cmd = Command::new(binary);
        cmd.arg("app-server");
        let (peer, mut incoming) = RpcPeer::spawn(cmd, false).map_err(SessionError::Spawn)?;
        let peer = Arc::new(peer);

        let handshake = async {
            peer.request(
                "initialize",
                json!({ "clientInfo": { "name": "rusi", "title": "Rusi", "version": env!("CARGO_PKG_VERSION") } }),
            )
            .await?;
            peer.notify("initialized", None).await?;

            let policy = policy_for(config);
            let mut params = json!({
                "approvalPolicy": policy.approval_policy,
                "sandbox": policy.sandbox,
            });
            if let Some(cwd) = &config.cwd {
                params["cwd"] = json!(cwd);
            }
            if let Some(model) = &config.model {
                params["model"] = json!(model);
            }
            let response = match &config.resume {
                Some(thread_id) => {
                    params["threadId"] = json!(thread_id);
                    params["excludeTurns"] = json!(true);
                    peer.request("thread/resume", params).await?
                }
                None => {
                    params["serviceName"] = json!("rusi");
                    peer.request("thread/start", params).await?
                }
            };
            Ok::<_, RpcError>(response)
        };
        let response = match handshake.await {
            Ok(r) => r,
            Err(err) => {
                peer.close().await;
                return Err(rpc_to_session(err));
            }
        };
        let thread_id = response["thread"]["id"]
            .as_str()
            .ok_or_else(|| SessionError::NotReady("o app-server não devolveu o id da thread".into()))?
            .to_string();

        let (tx, events) = mpsc::unbounded_channel();
        let _ = tx.send(AgentEvent::SessionStarted {
            session_id: thread_id.clone(),
            model: response["model"].as_str().map(str::to_string),
        });

        let turn_id = Arc::new(Mutex::new(None));
        let auto_accept = policy_for(config).auto_accept;
        let task_peer = peer.clone();
        let task_turn = turn_id.clone();
        tokio::spawn(async move {
            let mut translator = Translator::default();
            while let Some(message) = incoming.recv().await {
                match message {
                    Incoming::Notification { method, params } => {
                        if method == "turn/started" {
                            *task_turn.lock().unwrap_or_else(|p| p.into_inner()) =
                                params["turn"]["id"].as_str().map(str::to_string);
                        }
                        for event in translator.handle(&method, &params) {
                            if tx.send(event).is_err() {
                                return;
                            }
                        }
                    }
                    Incoming::Request { id, method, params } => {
                        let reply = answer_server_request(&method, &params, auto_accept);
                        if let Some(tool) = reply.denied {
                            translator.note_denied(tool);
                        }
                        let _ = match reply.result {
                            Ok(result) => task_peer.respond(id, result).await,
                            Err((code, message)) => task_peer.respond_error(id, code, message).await,
                        };
                    }
                    Incoming::Exited { stderr } => {
                        let _ = tx.send(AgentEvent::Error(
                            stderr.unwrap_or_else(|| "o app-server do codex terminou inesperadamente".into()),
                        ));
                        return;
                    }
                }
            }
        });

        Ok(Self { peer, thread_id, turn_id, events, effort: config.effort })
    }
}

#[async_trait]
impl AgentSession for CodexSession {
    async fn send(&mut self, prompt: &str) -> Result<(), SessionError> {
        let response = self
            .peer
            .request(
                "turn/start",
                json!({
                    "threadId": self.thread_id,
                    "input": [{ "type": "text", "text": prompt, "text_elements": [] }],
                    "effort": effort_name(self.effort),
                }),
            )
            .await
            .map_err(rpc_to_session)?;
        *self.turn_id.lock().unwrap_or_else(|p| p.into_inner()) = response["turn"]["id"].as_str().map(str::to_string);
        Ok(())
    }

    async fn next_event(&mut self) -> Option<AgentEvent> {
        self.events.recv().await
    }

    async fn interrupt(&mut self) -> Result<(), SessionError> {
        let turn_id = self.turn_id.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let Some(turn_id) = turn_id else { return Ok(()) };
        self.peer
            .request("turn/interrupt", json!({ "threadId": self.thread_id, "turnId": turn_id }))
            .await
            .map(|_| ())
            .map_err(rpc_to_session)
    }

    async fn close(&mut self) {
        self.peer.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(mode: AgentMode, skip: bool) -> SessionConfig {
        SessionConfig { mode, dangerously_skip_permissions: skip, ..Default::default() }
    }

    #[test]
    fn test_policy_mapping() {
        assert_eq!(policy_for(&cfg(AgentMode::Plan, false)).sandbox, "read-only");
        let accept = policy_for(&cfg(AgentMode::AcceptEdits, false));
        assert_eq!((accept.sandbox, accept.approval_policy, accept.auto_accept), ("workspace-write", "on-request", false));
        assert!(policy_for(&cfg(AgentMode::Auto, false)).auto_accept);
        let skip = policy_for(&cfg(AgentMode::Plan, true));
        assert_eq!((skip.sandbox, skip.approval_policy, skip.auto_accept), ("danger-full-access", "never", true));
    }

    #[test]
    fn test_deltas() {
        let mut t = Translator::default();
        assert_eq!(
            t.handle("item/agentMessage/delta", &json!({"delta": "oi"})),
            vec![AgentEvent::TextDelta("oi".into())]
        );
        assert_eq!(
            t.handle("item/reasoning/summaryTextDelta", &json!({"delta": "hm"})),
            vec![AgentEvent::ThinkingDelta("hm".into())]
        );
        assert!(t.handle("item/unknown", &json!({})).is_empty());
    }

    #[test]
    fn test_command_lifecycle() {
        let mut t = Translator::default();
        let started = t.handle(
            "item/started",
            &json!({"item": {"type": "commandExecution", "id": "c1", "command": "ls", "cwd": "/a", "status": "inProgress"}}),
        );
        assert_eq!(
            started,
            vec![AgentEvent::ToolUse { id: "c1".into(), name: "command".into(), input: json!({"command": "ls", "cwd": "/a"}) }]
        );
        let ok = t.handle(
            "item/completed",
            &json!({"item": {"type": "commandExecution", "id": "c1", "aggregatedOutput": "x", "exitCode": 0, "status": "completed"}}),
        );
        assert_eq!(ok, vec![AgentEvent::ToolResult { id: "c1".into(), output: "x".into(), is_error: false }]);
        let bad = t.handle(
            "item/completed",
            &json!({"item": {"type": "commandExecution", "id": "c2", "aggregatedOutput": "", "exitCode": 2, "status": "completed"}}),
        );
        assert!(matches!(&bad[0], AgentEvent::ToolResult { is_error: true, .. }));
    }

    #[test]
    fn test_file_change_and_mcp_items() {
        let mut t = Translator::default();
        let started = t.handle(
            "item/started",
            &json!({"item": {"type": "fileChange", "id": "f1", "changes": [{"path": "a.rs", "kind": "update", "diff": "..."}]}}),
        );
        assert_eq!(
            started,
            vec![AgentEvent::ToolUse {
                id: "f1".into(),
                name: "file_change".into(),
                input: json!({"changes": [{"path": "a.rs", "kind": "update"}]})
            }]
        );
        let mcp = t.handle(
            "item/started",
            &json!({"item": {"type": "mcpToolCall", "id": "m1", "server": "db", "tool": "query", "arguments": {"q": 1}}}),
        );
        assert!(matches!(&mcp[0], AgentEvent::ToolUse { name, .. } if name == "db/query"));
        let done = t.handle(
            "item/completed",
            &json!({"item": {"type": "mcpToolCall", "id": "m1", "status": "failed", "error": {"message": "boom"}}}),
        );
        assert_eq!(done, vec![AgentEvent::ToolResult { id: "m1".into(), output: "boom".into(), is_error: true }]);
    }

    #[test]
    fn test_turn_completion_aggregates_state() {
        let mut t = Translator::default();
        t.handle("item/completed", &json!({"item": {"type": "agentMessage", "id": "a", "text": "pronto"}}));
        t.handle("thread/tokenUsage/updated", &json!({"tokenUsage": {"last": {"inputTokens": 10, "outputTokens": 3, "cachedInputTokens": 4}}}));
        t.handle("thread/tokenUsage/updated", &json!({"tokenUsage": {"last": {"inputTokens": 5, "outputTokens": 2, "cachedInputTokens": 0}}}));
        t.note_denied("command");
        let done = t.handle("turn/completed", &json!({"turn": {"id": "t", "status": "completed", "durationMs": 42}}));
        assert_eq!(
            done,
            vec![AgentEvent::TurnComplete {
                text: Some("pronto".into()),
                is_error: false,
                usage: Some(TokenUsage { input_tokens: 15, output_tokens: 5, cache_read_tokens: 4, cache_creation_tokens: 0 }),
                cost_usd: None,
                duration_ms: Some(42),
                denied_tools: vec!["command".into()],
            }]
        );
        // O estado é zerado para o turno seguinte.
        let next = t.handle("turn/completed", &json!({"turn": {"status": "completed"}}));
        assert!(matches!(&next[0], AgentEvent::TurnComplete { text: None, usage: None, denied_tools, .. } if denied_tools.is_empty()));
    }

    #[test]
    fn test_failed_and_interrupted_turns_are_errors() {
        let mut t = Translator::default();
        t.handle("error", &json!({"error": {"message": "sem quota"}, "willRetry": false}));
        let failed = t.handle("turn/completed", &json!({"turn": {"status": "failed", "error": null}}));
        assert!(matches!(&failed[0], AgentEvent::TurnComplete { is_error: true, text: Some(m), .. } if m == "sem quota"));
        let interrupted = t.handle("turn/completed", &json!({"turn": {"status": "interrupted"}}));
        assert!(matches!(&interrupted[0], AgentEvent::TurnComplete { is_error: true, .. }));
    }

    #[test]
    fn test_approval_answers() {
        let deny = answer_server_request("item/commandExecution/requestApproval", &json!({}), false);
        assert_eq!(deny.result, Ok(json!({"decision": "decline"})));
        assert_eq!(deny.denied, Some("command"));
        let accept = answer_server_request("item/fileChange/requestApproval", &json!({}), true);
        assert_eq!(accept.result, Ok(json!({"decision": "accept"})));
        assert_eq!(accept.denied, None);

        let legacy = answer_server_request("execCommandApproval", &json!({}), false);
        assert_eq!(legacy.result, Ok(json!({"decision": "denied"})));

        let perms = answer_server_request("item/permissions/requestApproval", &json!({"permissions": {"network": true}}), true);
        assert_eq!(perms.result, Ok(json!({"permissions": {"network": true}, "scope": "turn"})));

        assert!(answer_server_request("account/chatgptAuthTokens/refresh", &json!({}), true).result.is_err());
        assert!(answer_server_request("desconhecido", &json!({}), true).result.is_err());
    }
}
