//! Sessão com `muse serve` (MSP, JSON-RPC 2.0 sobre stdio): `initialize`, `session/start|resume`,
//! `turn/start`, `turn/interrupt`, notificações `item/*`, `turn/completed` e
//! aprovações via `approval/requested` + `approval/decide`.

use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::config::{AgentMode, EffortLevel};
use crate::rpc::{Incoming, RpcError, RpcPeer};
use crate::session::{AgentEvent, AgentSession, ApprovalRequest, PendingApprovals, SessionConfig, SessionError, TokenUsage};

fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Política traduzida do modo do Rusi para o vocabulário do Muse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MusePolicy {
    pub approval_mode: &'static str,
    pub permission_profile: &'static str,
    pub disable_sandbox: bool,
    pub auto_accept: bool,
    /// Pedidos viram `ApprovalRequested` e quem decide é o usuário.
    pub ask: bool,
}

pub(super) fn policy_for(config: &SessionConfig) -> MusePolicy {
    if config.dangerously_skip_permissions {
        return MusePolicy {
            approval_mode: "allowAll",
            permission_profile: ":unrestricted",
            disable_sandbox: true,
            auto_accept: true,
            ask: false,
        };
    }
    match config.mode {
        AgentMode::Plan => MusePolicy {
            approval_mode: "denyUnmatched",
            permission_profile: ":ask-me",
            disable_sandbox: false,
            auto_accept: false,
            ask: false,
        },
        AgentMode::AcceptEdits => MusePolicy {
            approval_mode: "promptUnmatched",
            permission_profile: ":ask-me",
            disable_sandbox: false,
            auto_accept: false,
            ask: true,
        },
        AgentMode::Auto => MusePolicy {
            approval_mode: "allowAll",
            permission_profile: ":ask-me",
            disable_sandbox: false,
            auto_accept: true,
            ask: false,
        },
    }
}

fn decide_params(session_id: &str, params: &Value, choice_id: String) -> Value {
    json!({
        "commandId": new_id(),
        "sessionId": session_id,
        "approvalId": params["approvalId"],
        "requirementId": params["currentRequirementId"],
        "choiceId": choice_id,
    })
}

fn effort_name(effort: EffortLevel) -> &'static str {
    match effort {
        EffortLevel::Low => "low",
        EffortLevel::Medium => "medium",
        EffortLevel::High => "high",
    }
}

/// Escolhe a opção de um `approval/requested`: aprovar (uma vez) quando `auto_accept`,
/// senão negar. Devolve o `choiceId` e o nome da ferramenta, se foi negada.
pub(super) fn pick_choice(params: &Value, auto_accept: bool) -> Option<(String, Option<String>)> {
    let choices = params["availableChoices"].as_array()?;
    let find = |decisions: &[&str]| {
        decisions.iter().find_map(|wanted| {
            choices
                .iter()
                .find(|c| c["decision"] == *wanted && c["scope"].as_str().is_none_or(|s| s == "once"))
        })
    };
    let chosen = if auto_accept { find(&["approved"]) } else { find(&["denied", "abort"]) }
        .or_else(|| choices.iter().find(|c| c["decision"] == if auto_accept { "approved" } else { "denied" }))?;
    let tool = (!auto_accept).then(|| params["toolName"].as_str().unwrap_or("tool").to_string());
    Some((chosen["choiceId"].as_str()?.to_string(), tool))
}

/// Estado para traduzir as notificações do Muse em eventos normalizados.
#[derive(Debug, Default)]
pub(super) struct Translator {
    kinds: HashMap<String, String>,
    tools_emitted: HashSet<String>,
    final_text: Option<String>,
    usage: Option<TokenUsage>,
    denied: Vec<String>,
}

impl Translator {
    pub(super) fn note_denied(&mut self, tool: &str) {
        self.denied.push(tool.to_string());
    }

    pub(super) fn handle(&mut self, method: &str, params: &Value) -> Vec<AgentEvent> {
        match method {
            "item/started" | "item/updated" | "item/completed" => self.handle_item(method, &params["item"]),
            "item/delta" => self.handle_delta(params),
            "session/tokenUsage" => {
                let u = &params["usage"];
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                let usage = self.usage.get_or_insert_with(TokenUsage::default);
                usage.input_tokens += n("inputTokens");
                usage.output_tokens += n("outputTokens");
                usage.cache_read_tokens += n("cacheReadTokens").max(n("cachedTokens"));
                usage.cache_creation_tokens += n("cacheWriteTokens");
                Vec::new()
            }
            "turn/completed" => vec![self.finish_turn(params)],
            "session/closed" => vec![AgentEvent::Error(format!(
                "a sessão do Muse foi encerrada ({})",
                params["reason"].as_str().unwrap_or("motivo desconhecido")
            ))],
            _ => Vec::new(),
        }
    }

    fn handle_item(&mut self, method: &str, item: &Value) -> Vec<AgentEvent> {
        let (Some(id), Some(kind)) = (item["itemId"].as_str(), item["kind"].as_str()) else {
            return Vec::new();
        };
        self.kinds.insert(id.to_string(), kind.to_string());
        let status = item["status"].as_str().unwrap_or("");
        let mut events = Vec::new();

        match kind {
            "agentMessage" if method == "item/completed" => {
                if let Some(text) = item["text"].as_str().filter(|t| !t.is_empty()) {
                    self.final_text = Some(text.to_string());
                }
            }
            "toolCall" | "userShell" | "webSearch" => {
                let tool = item["tool"].as_str().unwrap_or(match kind {
                    "userShell" => "shell",
                    "webSearch" => "web_search",
                    _ => "tool",
                });
                if self.tools_emitted.insert(id.to_string()) {
                    let args = item["args"].as_str().or(item["commandText"].as_str()).unwrap_or("");
                    let input = serde_json::from_str(args).unwrap_or_else(|_| json!(args));
                    events.push(AgentEvent::ToolUse { id: id.to_string(), name: tool.to_string(), input });
                }
                if method == "item/completed" || (status != "inProgress" && !status.is_empty()) {
                    let failed = status != "completed" || item["exitCode"].as_i64().is_some_and(|c| c != 0);
                    let output = item["visibleOutput"]
                        .as_str()
                        .or(item["failureReason"].as_str())
                        .unwrap_or("")
                        .to_string();
                    events.push(AgentEvent::ToolResult { id: id.to_string(), output, is_error: failed });
                }
            }
            _ => {}
        }
        events
    }

    fn handle_delta(&self, params: &Value) -> Vec<AgentEvent> {
        let (Some(id), Some(delta)) = (params["itemId"].as_str(), params["delta"].as_str()) else {
            return Vec::new();
        };
        match self.kinds.get(id).map(String::as_str) {
            Some("agentMessage") => vec![AgentEvent::TextDelta(delta.to_string())],
            Some("reasoning") => vec![AgentEvent::ThinkingDelta(delta.to_string())],
            _ => Vec::new(),
        }
    }

    fn finish_turn(&mut self, params: &Value) -> AgentEvent {
        let terminal = params["terminal"].as_str().unwrap_or("completed");
        let error = params["error"]["message"].as_str().map(str::to_string);
        let text = self.final_text.take().or(error);
        AgentEvent::TurnComplete {
            text,
            is_error: terminal != "completed",
            usage: self.usage.take(),
            cost_usd: None,
            duration_ms: params["durationMs"].as_u64(),
            denied_tools: std::mem::take(&mut self.denied),
        }
    }
}

pub struct MuseSession {
    peer: Arc<RpcPeer>,
    session_id: String,
    cwd: String,
    turn_id: Arc<Mutex<Option<String>>>,
    events: mpsc::UnboundedReceiver<AgentEvent>,
    effort: EffortLevel,
    approvals: PendingApprovals,
}

fn rpc_to_session(err: RpcError) -> SessionError {
    match err {
        RpcError::Closed(detail) => SessionError::Closed(detail),
        RpcError::Remote { message, .. } => SessionError::NotReady(message),
    }
}

impl MuseSession {
    pub(super) async fn start(binary: &Path, config: &SessionConfig) -> Result<Self, SessionError> {
        let policy = policy_for(config);
        let mut cmd = Command::new(binary);
        cmd.arg("serve").arg("--trust-workspace");
        if policy.disable_sandbox {
            cmd.arg("--disable-sandbox");
        }
        cmd.env("MUSE_NO_AUTO_UPDATE", "1");
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }
        let cwd = config
            .cwd
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".".into());

        let (peer, mut incoming) = RpcPeer::spawn(cmd, true).map_err(SessionError::Spawn)?;
        let peer = Arc::new(peer);
        let session_id = config.resume.clone().unwrap_or_else(new_id);

        let handshake = async {
            peer.request(
                "initialize",
                json!({
                    "clientInfo": { "name": "rusi", "title": "Rusi", "version": env!("CARGO_PKG_VERSION") },
                    "capabilities": {},
                }),
            )
            .await?;
            peer.notify("initialized", None).await?;
            if config.resume.is_some() {
                peer.request(
                    "session/resume",
                    json!({
                        "commandId": new_id(),
                        "sessionId": session_id,
                        "excludeItems": true,
                        "permissionProfile": policy.permission_profile,
                    }),
                )
                .await?;
                peer.request(
                    "session/setApprovalMode",
                    json!({ "commandId": new_id(), "sessionId": session_id, "mode": policy.approval_mode }),
                )
                .await
            } else {
                let mut params = json!({
                    "commandId": new_id(),
                    "sessionId": session_id,
                    "workspaceRoot": cwd,
                    "providerId": "meta",
                    "approvalMode": policy.approval_mode,
                    "permissionProfile": policy.permission_profile,
                });
                if let Some(model) = &config.model {
                    params["modelId"] = json!(model);
                }
                peer.request("session/start", params).await
            }
        };
        let started = match handshake.await {
            Ok(v) => v,
            Err(err) => {
                peer.close().await;
                return Err(rpc_to_session(err));
            }
        };

        let (tx, events) = mpsc::unbounded_channel();
        let _ = tx.send(AgentEvent::SessionStarted {
            session_id: session_id.clone(),
            model: started["session"]["modelId"].as_str().map(str::to_string).or(config.model.clone()),
        });

        let turn_id: Arc<Mutex<Option<String>>> = Arc::default();
        let task_peer = peer.clone();
        let task_session = session_id.clone();
        let (auto_accept, ask) = (policy.auto_accept, policy.ask);
        let approvals = PendingApprovals::default();
        let task_approvals = approvals.clone();
        tokio::spawn(async move {
            let mut translator = Translator::default();
            while let Some(message) = incoming.recv().await {
                match message {
                    Incoming::Notification { method, params } => {
                        match method.as_str() {
                            "approval/requested" if ask => {
                                // Sem `approvalId` não há como responder; o Muse segue sem resposta.
                                if let Some(key) = params["approvalId"].as_str().map(str::to_string) {
                                    let (peer, session, reply_params) = (task_peer.clone(), task_session.clone(), params.clone());
                                    task_approvals.register(key.clone(), move |approved| async move {
                                        if let Some((choice_id, _)) = pick_choice(&reply_params, approved) {
                                            let _ = peer.request("approval/decide", decide_params(&session, &reply_params, choice_id)).await;
                                        }
                                    });
                                    let request = ApprovalRequest {
                                        id: key,
                                        tool: params["toolName"].as_str().unwrap_or("tool").to_string(),
                                        input: params.clone(),
                                    };
                                    if tx.send(AgentEvent::ApprovalRequested(request)).is_err() {
                                        return;
                                    }
                                }
                            }
                            "approval/requested" => {
                                if let Some((choice_id, denied)) = pick_choice(&params, auto_accept) {
                                    if let Some(tool) = denied {
                                        translator.note_denied(&tool);
                                    }
                                    let _ = task_peer.request("approval/decide", decide_params(&task_session, &params, choice_id)).await;
                                }
                            }
                            "userInput/requested" => {
                                let _ = task_peer
                                    .request(
                                        "userInput/cancel",
                                        json!({
                                            "commandId": new_id(),
                                            "sessionId": task_session,
                                            "userInputId": params["userInputId"],
                                        }),
                                    )
                                    .await;
                            }
                            _ => {}
                        }
                        for event in translator.handle(&method, &params) {
                            if tx.send(event).is_err() {
                                return;
                            }
                        }
                    }
                    Incoming::Request { id, method, .. } => {
                        // As informações vêm nas notificações `*/requested`; aqui só se confirma.
                        let _ = if matches!(method.as_str(), "approval/request" | "userInput/request") {
                            task_peer.respond(id, json!({})).await
                        } else {
                            task_peer.respond_error(id, -32601, "método não suportado").await
                        };
                    }
                    Incoming::Exited { stderr } => {
                        let _ = tx.send(AgentEvent::Error(
                            stderr.unwrap_or_else(|| "o muse serve terminou inesperadamente".into()),
                        ));
                        return;
                    }
                }
            }
        });

        Ok(Self { peer, session_id, cwd, turn_id, events, effort: config.effort, approvals })
    }
}

#[async_trait]
impl AgentSession for MuseSession {
    async fn send(&mut self, prompt: &str) -> Result<(), SessionError> {
        let turn_id = new_id();
        *self.turn_id.lock().unwrap_or_else(|p| p.into_inner()) = Some(turn_id.clone());
        let preview: String = prompt.chars().take(80).collect();
        self.peer
            .request(
                "turn/start",
                json!({
                    "commandId": turn_id,
                    "sessionId": self.session_id,
                    "input": [{ "type": "text", "text": prompt }],
                    "displayText": preview,
                    "workspaceRoots": [self.cwd],
                    "ifBusy": "queue",
                    "reasoningEffort": effort_name(self.effort),
                }),
            )
            .await
            .map(|_| ())
            .map_err(rpc_to_session)
    }

    async fn next_event(&mut self) -> Option<AgentEvent> {
        self.events.recv().await
    }

    async fn respond_approval(&mut self, id: &str, approved: bool) -> Result<(), SessionError> {
        self.approvals.resolve(id, approved).await
    }

    async fn interrupt(&mut self) -> Result<(), SessionError> {
        let turn_id = self.turn_id.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let Some(turn_id) = turn_id else { return Ok(()) };
        self.peer
            .request(
                "turn/interrupt",
                json!({ "commandId": new_id(), "sessionId": self.session_id, "turnId": turn_id }),
            )
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
        assert_eq!(policy_for(&cfg(AgentMode::Plan, false)).approval_mode, "denyUnmatched");
        let accept = policy_for(&cfg(AgentMode::AcceptEdits, false));
        assert_eq!((accept.approval_mode, accept.auto_accept, accept.ask), ("promptUnmatched", false, true));
        assert!(!policy_for(&cfg(AgentMode::Plan, false)).ask);
        assert!(policy_for(&cfg(AgentMode::Auto, false)).auto_accept);
        let skip = policy_for(&cfg(AgentMode::Plan, true));
        assert_eq!((skip.permission_profile, skip.disable_sandbox), (":unrestricted", true));
    }

    #[test]
    fn test_pick_choice_prefers_once_scope() {
        let params = json!({"toolName": "shell", "availableChoices": [
            {"choiceId": "s", "decision": "approvedForSession", "scope": "session"},
            {"choiceId": "a-sess", "decision": "approved", "scope": "session"},
            {"choiceId": "a", "decision": "approved", "scope": "once"},
            {"choiceId": "x", "decision": "abort", "scope": "once"},
        ]});
        assert_eq!(pick_choice(&params, true), Some(("a".into(), None)));
        // Só há `abort` para negar (como nos shells do Muse).
        assert_eq!(pick_choice(&params, false), Some(("x".into(), Some("shell".into()))));
        assert_eq!(pick_choice(&json!({}), true), None);
    }

    #[test]
    fn test_text_and_reasoning_deltas_follow_item_kind() {
        let mut t = Translator::default();
        t.handle("item/started", &json!({"item": {"itemId": "m", "kind": "agentMessage", "status": "inProgress"}}));
        t.handle("item/started", &json!({"item": {"itemId": "r", "kind": "reasoning", "status": "inProgress"}}));
        assert_eq!(t.handle("item/delta", &json!({"itemId": "m", "delta": "oi", "field": "text"})), vec![AgentEvent::TextDelta("oi".into())]);
        assert_eq!(t.handle("item/delta", &json!({"itemId": "r", "delta": "hm"})), vec![AgentEvent::ThinkingDelta("hm".into())]);
        assert!(t.handle("item/delta", &json!({"itemId": "desconhecido", "delta": "x"})).is_empty());
    }

    #[test]
    fn test_tool_call_emits_use_once_and_result_on_completion() {
        let mut t = Translator::default();
        let started = t.handle(
            "item/started",
            &json!({"item": {"itemId": "t1", "kind": "toolCall", "tool": "shell", "args": "{\"command\":\"ls\"}", "status": "inProgress"}}),
        );
        assert_eq!(
            started,
            vec![AgentEvent::ToolUse { id: "t1".into(), name: "shell".into(), input: json!({"command": "ls"}) }]
        );
        assert!(t.handle("item/updated", &json!({"item": {"itemId": "t1", "kind": "toolCall", "tool": "shell", "status": "inProgress"}})).is_empty());
        let done = t.handle(
            "item/completed",
            &json!({"item": {"itemId": "t1", "kind": "toolCall", "tool": "shell", "status": "completed", "exitCode": 1, "visibleOutput": "erro"}}),
        );
        assert_eq!(done, vec![AgentEvent::ToolResult { id: "t1".into(), output: "erro".into(), is_error: true }]);
    }

    #[test]
    fn test_turn_completion() {
        let mut t = Translator::default();
        t.handle("item/completed", &json!({"item": {"itemId": "m", "kind": "agentMessage", "status": "completed", "text": "pronto"}}));
        t.handle("session/tokenUsage", &json!({"usage": {"inputTokens": 10, "outputTokens": 2, "cachedTokens": 4}}));
        t.note_denied("shell");
        let done = t.handle("turn/completed", &json!({"terminal": "completed", "durationMs": 9}));
        assert_eq!(
            done,
            vec![AgentEvent::TurnComplete {
                text: Some("pronto".into()),
                is_error: false,
                usage: Some(TokenUsage { input_tokens: 10, output_tokens: 2, cache_read_tokens: 4, cache_creation_tokens: 0 }),
                cost_usd: None,
                duration_ms: Some(9),
                denied_tools: vec!["shell".into()],
            }]
        );
        let failed = t.handle("turn/completed", &json!({"terminal": "failed", "error": {"message": "sem quota"}}));
        assert!(matches!(&failed[0], AgentEvent::TurnComplete { is_error: true, text: Some(m), .. } if m == "sem quota"));
    }

    #[test]
    fn test_session_closed_is_an_error() {
        let mut t = Translator::default();
        assert!(matches!(&t.handle("session/closed", &json!({"reason": "hostShutdown"}))[0], AgentEvent::Error(m) if m.contains("hostShutdown")));
    }
}
