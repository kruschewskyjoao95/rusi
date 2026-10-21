//! Sessão sobre o Agent Client Protocol (JSON-RPC 2.0 em stdio). No T3 só o Antigravity e o
//! Grok usam ACP; cada um fornece aqui o seu binário, argumentos, ambiente e regras de login.

use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::rpc::{Incoming, RpcError, RpcPeer};
use crate::session::{AgentEvent, AgentSession, ApprovalRequest, PendingApprovals, SessionConfig, SessionError, TokenUsage};

const AUTH_TIMEOUT: Duration = Duration::from_secs(30);
const PROMPT_DONE: &str = "$/promptDone";
/// Código de erro ACP "authentication required".
const AUTH_REQUIRED_CODE: i64 = -32000;

/// Como iniciar e autenticar um agente ACP específico.
pub(super) struct AcpOptions {
    pub binary: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// `_meta` do `initialize`.
    pub init_meta: Option<Value>,
    /// Responde `allow_*` aos pedidos de permissão (senão `reject_*`).
    pub auto_accept: bool,
    /// Entra no modo `plan` do agente, se ele o anunciar.
    pub plan_mode: bool,
    /// Escolhe o método de `authenticate` quando o agente exige login, a partir do resultado
    /// de `initialize`; `None` significa que o login só pode ser feito pelo CLI do provedor.
    pub auth_method: fn(&Value) -> Option<String>,
    /// Mensagem quando não há como autenticar a partir daqui.
    pub login_hint: String,
}

fn permission_tool(params: &Value) -> String {
    params["toolCall"]["title"].as_str().or(params["toolCall"]["kind"].as_str()).unwrap_or("tool").to_string()
}

/// Resposta a um `session/request_permission`.
fn permission_reply(params: &Value, accepted: bool) -> Value {
    match pick_permission_option(params, accepted) {
        Some(option_id) => json!({ "outcome": { "outcome": "selected", "optionId": option_id } }),
        None => json!({ "outcome": { "outcome": "cancelled" } }),
    }
}

/// Escolhe a opção de um `session/request_permission`. Devolve o `optionId`, ou `None`
/// para responder `cancelled`.
pub(super) fn pick_permission_option(params: &Value, auto_accept: bool) -> Option<String> {
    let options = params["options"].as_array()?;
    let preferred: &[&str] = if auto_accept {
        &["allow_once", "allow_always"]
    } else {
        &["reject_once", "reject_always"]
    };
    preferred
        .iter()
        .find_map(|kind| options.iter().find(|o| o["kind"] == *kind))
        .and_then(|o| o["optionId"].as_str().map(str::to_string))
}

fn text_of_content(content: &Value) -> String {
    match content {
        Value::Array(items) => items.iter().map(text_of_content).collect::<Vec<_>>().join(""),
        Value::Object(_) => {
            // `{type:"content", content:{type:"text", text}}` ou bloco de texto direto.
            if let Some(text) = content["text"].as_str() {
                text.to_string()
            } else {
                text_of_content(&content["content"])
            }
        }
        _ => String::new(),
    }
}

#[derive(Debug, Default)]
pub(super) struct Translator {
    text: String,
    denied: Vec<String>,
}

impl Translator {
    pub(super) fn note_denied(&mut self, tool: &str) {
        self.denied.push(tool.to_string());
    }

    pub(super) fn handle(&mut self, method: &str, params: &Value) -> Vec<AgentEvent> {
        match method {
            "session/update" => self.handle_update(&params["update"]),
            PROMPT_DONE => vec![self.finish_turn(params)],
            _ => Vec::new(),
        }
    }

    fn handle_update(&mut self, update: &Value) -> Vec<AgentEvent> {
        match update["sessionUpdate"].as_str() {
            Some("agent_message_chunk") => {
                let text = text_of_content(&update["content"]);
                if text.is_empty() {
                    return Vec::new();
                }
                self.text.push_str(&text);
                vec![AgentEvent::TextDelta(text)]
            }
            Some("agent_thought_chunk") => {
                let text = text_of_content(&update["content"]);
                (!text.is_empty()).then_some(AgentEvent::ThinkingDelta(text)).into_iter().collect()
            }
            Some("tool_call") => update["toolCallId"]
                .as_str()
                .map(|id| AgentEvent::ToolUse {
                    id: id.to_string(),
                    name: update["title"].as_str().or(update["kind"].as_str()).unwrap_or("tool").to_string(),
                    input: update["rawInput"].clone(),
                })
                .into_iter()
                .collect(),
            Some("tool_call_update") => {
                let (Some(id), Some(status)) = (update["toolCallId"].as_str(), update["status"].as_str()) else {
                    return Vec::new();
                };
                if !matches!(status, "completed" | "failed") {
                    return Vec::new();
                }
                let mut output = text_of_content(&update["content"]);
                if output.is_empty() && !update["rawOutput"].is_null() {
                    output = match &update["rawOutput"] {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                }
                vec![AgentEvent::ToolResult { id: id.to_string(), output, is_error: status == "failed" }]
            }
            _ => Vec::new(),
        }
    }

    /// `params`: `{ result: <resposta de session/prompt> }` ou `{ error: "mensagem" }`.
    fn finish_turn(&mut self, params: &Value) -> AgentEvent {
        let text = std::mem::take(&mut self.text);
        let denied_tools = std::mem::take(&mut self.denied);
        if let Some(error) = params["error"].as_str() {
            return AgentEvent::TurnComplete {
                text: Some(error.to_string()),
                is_error: true,
                usage: None,
                cost_usd: None,
                duration_ms: None,
                denied_tools,
            };
        }
        let result = &params["result"];
        let stop = result["stopReason"].as_str().unwrap_or("end_turn");
        let usage = result["usage"].as_object().map(|_| {
            let n = |k: &str| result["usage"][k].as_u64().unwrap_or(0);
            TokenUsage {
                input_tokens: n("inputTokens"),
                output_tokens: n("outputTokens"),
                cache_read_tokens: n("cachedReadTokens"),
                cache_creation_tokens: n("cachedWriteTokens"),
            }
        });
        let (text, is_error) = match stop {
            "cancelled" => (Some(text).filter(|t| !t.is_empty()), true),
            "refusal" => (Some(if text.is_empty() { "o agente recusou o pedido".to_string() } else { text }), true),
            _ => (Some(text).filter(|t| !t.is_empty()), false),
        };
        AgentEvent::TurnComplete { text, is_error, usage, cost_usd: None, duration_ms: None, denied_tools }
    }
}

pub struct AcpSession {
    peer: Arc<RpcPeer>,
    session_id: String,
    events: mpsc::UnboundedReceiver<AgentEvent>,
    approvals: PendingApprovals,
}

fn rpc_to_session(err: RpcError) -> SessionError {
    match err {
        RpcError::Closed(detail) => SessionError::Closed(detail),
        RpcError::Remote { message, .. } => SessionError::NotReady(message),
    }
}

fn is_auth_required(err: &RpcError) -> bool {
    matches!(err, RpcError::Remote { code, message }
        if *code == AUTH_REQUIRED_CODE && message.to_lowercase().contains("auth"))
}

/// Abre a conversa: nova, ou retomada por `session/load`/`session/resume` conforme o agente suporta.
async fn open_session(
    peer: &RpcPeer,
    config: &SessionConfig,
    initialized: &Value,
    cwd: &str,
) -> Result<Value, RpcError> {
    let caps = &initialized["agentCapabilities"];
    let can_resume = !caps["sessionCapabilities"]["resume"].is_null();
    let can_load = caps["loadSession"] == true;
    let (method, params) = match &config.resume {
        Some(id) if can_load => {
            ("session/load", json!({ "sessionId": id, "cwd": cwd, "mcpServers": [] }))
        }
        Some(id) if can_resume => {
            ("session/resume", json!({ "sessionId": id, "cwd": cwd, "mcpServers": [] }))
        }
        Some(_) => {
            return Err(RpcError::Remote {
                code: -32601,
                message: "o agente não anuncia session/load nem session/resume".into(),
            });
        }
        None => ("session/new", json!({ "cwd": cwd, "mcpServers": [] })),
    };
    peer.request(method, params).await
}

impl AcpSession {
    pub(super) async fn start(options: AcpOptions, config: &SessionConfig) -> Result<Self, SessionError> {
        let mut cmd = Command::new(&options.binary);
        cmd.args(&options.args);
        for (key, value) in &options.env {
            cmd.env(key, value);
        }
        let cwd = config
            .cwd
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".".into());
        cmd.current_dir(&cwd);

        let (peer, mut incoming) = RpcPeer::spawn(cmd, true).map_err(SessionError::Spawn)?;
        let peer = Arc::new(peer);

        let setup = Self::handshake(&peer, &options, config, &cwd).await;
        let (session_id, model) = match setup {
            Ok(v) => v,
            Err(err) => {
                peer.close().await;
                return Err(err);
            }
        };

        let (tx, events) = mpsc::unbounded_channel();
        let _ = tx.send(AgentEvent::SessionStarted { session_id: session_id.clone(), model });

        let task_peer = peer.clone();
        let auto_accept = options.auto_accept;
        // O modo plan do agente nega por conta própria; fora dele e sem auto-aceite, o usuário decide.
        let interactive = !auto_accept && !options.plan_mode;
        let approvals = PendingApprovals::default();
        let task_approvals = approvals.clone();
        tokio::spawn(async move {
            let mut translator = Translator::default();
            while let Some(message) = incoming.recv().await {
                match message {
                    Incoming::Notification { method, params } => {
                        for event in translator.handle(&method, &params) {
                            if tx.send(event).is_err() {
                                return;
                            }
                        }
                    }
                    Incoming::Request { id, method, params } if method == "session/request_permission" => {
                        let tool = permission_tool(&params);
                        if interactive {
                            let key = id.to_string();
                            let peer = task_peer.clone();
                            let reply_params = params.clone();
                            task_approvals.register(key.clone(), move |approved| async move {
                                let _ = peer.respond(id, permission_reply(&reply_params, approved)).await;
                            });
                            let request = ApprovalRequest { id: key, tool, input: params["toolCall"].clone() };
                            if tx.send(AgentEvent::ApprovalRequested(request)).is_err() {
                                return;
                            }
                            continue;
                        }
                        if !auto_accept {
                            translator.note_denied(&tool);
                        }
                        let _ = task_peer.respond(id, permission_reply(&params, auto_accept)).await;
                    }
                    Incoming::Request { id, .. } => {
                        // fs/* e terminal/* não são oferecidos (capabilities vazias).
                        let _ = task_peer.respond_error(id, -32601, "método não suportado").await;
                    }
                    Incoming::Exited { stderr } => {
                        let _ = tx.send(AgentEvent::Error(
                            stderr.unwrap_or_else(|| "o agente ACP terminou inesperadamente".into()),
                        ));
                        return;
                    }
                }
            }
        });

        Ok(Self { peer, session_id, events, approvals })
    }

    /// `initialize` → (login se exigido) → `session/new|load|resume` → modo/modelo.
    async fn handshake(
        peer: &RpcPeer,
        options: &AcpOptions,
        config: &SessionConfig,
        cwd: &str,
    ) -> Result<(String, Option<String>), SessionError> {
        let mut init = json!({
            "protocolVersion": 1,
            "clientCapabilities": {},
            "clientInfo": { "name": "rusi", "title": "Rusi", "version": env!("CARGO_PKG_VERSION") },
        });
        if let Some(meta) = &options.init_meta {
            init["_meta"] = meta.clone();
        }
        let initialized = peer.request("initialize", init).await.map_err(rpc_to_session)?;

        let opened = match open_session(peer, config, &initialized, cwd).await {
            Err(err) if is_auth_required(&err) => {
                let Some(method_id) = (options.auth_method)(&initialized) else {
                    return Err(SessionError::NotReady(options.login_hint.clone()));
                };
                tokio::time::timeout(AUTH_TIMEOUT, peer.request("authenticate", json!({ "methodId": method_id })))
                    .await
                    .map_err(|_| SessionError::NotReady(options.login_hint.clone()))?
                    .map_err(rpc_to_session)?;
                open_session(peer, config, &initialized, cwd).await.map_err(rpc_to_session)?
            }
            other => other.map_err(rpc_to_session)?,
        };

        let session_id = match &config.resume {
            Some(id) => id.clone(),
            None => opened["sessionId"]
                .as_str()
                .ok_or_else(|| SessionError::NotReady("o agente não devolveu o id da sessão".into()))?
                .to_string(),
        };

        let plan_advertised = opened["modes"]["availableModes"]
            .as_array()
            .is_some_and(|modes| modes.iter().any(|m| m["id"] == "plan"));
        if options.plan_mode && plan_advertised {
            let _ = peer.request("session/set_mode", json!({ "sessionId": session_id, "modeId": "plan" })).await;
        }
        if let Some(model) = &config.model {
            let _ = peer.request("session/set_model", json!({ "sessionId": session_id, "modelId": model })).await;
        }
        let model = config.model.clone().or_else(|| opened["models"]["currentModelId"].as_str().map(str::to_string));
        Ok((session_id, model))
    }
}

#[async_trait]
impl AgentSession for AcpSession {
    async fn send(&mut self, prompt: &str) -> Result<(), SessionError> {
        let peer = self.peer.clone();
        let params = json!({
            "sessionId": self.session_id,
            "prompt": [{ "type": "text", "text": prompt }],
        });
        tokio::spawn(async move {
            let done = match peer.request("session/prompt", params).await {
                Ok(result) => json!({ "result": result }),
                Err(RpcError::Closed(_)) => return, // `Exited` reporta o fim do processo
                Err(err) => json!({ "error": err.to_string() }),
            };
            peer.inject(Incoming::Notification { method: PROMPT_DONE.into(), params: done });
        });
        Ok(())
    }

    async fn next_event(&mut self) -> Option<AgentEvent> {
        self.events.recv().await
    }

    async fn respond_approval(&mut self, id: &str, approved: bool) -> Result<(), SessionError> {
        self.approvals.resolve(id, approved).await
    }

    async fn interrupt(&mut self) -> Result<(), SessionError> {
        self.peer
            .notify("session/cancel", Some(json!({ "sessionId": self.session_id })))
            .await
            .map_err(rpc_to_session)
    }

    async fn close(&mut self) {
        self.peer.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_option_choice() {
        let params = json!({"options": [
            {"optionId": "aa", "kind": "allow_always"},
            {"optionId": "ao", "kind": "allow_once"},
            {"optionId": "ro", "kind": "reject_once"},
        ]});
        assert_eq!(pick_permission_option(&params, true).as_deref(), Some("ao"));
        assert_eq!(pick_permission_option(&params, false).as_deref(), Some("ro"));
        let only_allow = json!({"options": [{"optionId": "aa", "kind": "allow_always"}]});
        assert_eq!(pick_permission_option(&only_allow, true).as_deref(), Some("aa"));
        assert_eq!(pick_permission_option(&only_allow, false), None);
    }

    #[test]
    fn test_message_and_thought_chunks() {
        let mut t = Translator::default();
        let msg = t.handle("session/update", &json!({"update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "oi"}}}));
        assert_eq!(msg, vec![AgentEvent::TextDelta("oi".into())]);
        let thought = t.handle("session/update", &json!({"update": {"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "hm"}}}));
        assert_eq!(thought, vec![AgentEvent::ThinkingDelta("hm".into())]);
        assert!(t.handle("session/update", &json!({"update": {"sessionUpdate": "plan", "entries": []}})).is_empty());
    }

    #[test]
    fn test_tool_call_lifecycle() {
        let mut t = Translator::default();
        let call = t.handle("session/update", &json!({"update": {"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Run ls", "kind": "execute", "rawInput": {"command": "ls"}}}));
        assert_eq!(call, vec![AgentEvent::ToolUse { id: "t1".into(), name: "Run ls".into(), input: json!({"command": "ls"}) }]);
        assert!(t.handle("session/update", &json!({"update": {"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "in_progress"}})).is_empty());
        let done = t.handle("session/update", &json!({"update": {"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed", "content": [{"type": "content", "content": {"type": "text", "text": "a.rs"}}]}}));
        assert_eq!(done, vec![AgentEvent::ToolResult { id: "t1".into(), output: "a.rs".into(), is_error: false }]);
        let failed = t.handle("session/update", &json!({"update": {"sessionUpdate": "tool_call_update", "toolCallId": "t2", "status": "failed", "rawOutput": {"err": "x"}}}));
        assert!(matches!(&failed[0], AgentEvent::ToolResult { is_error: true, output, .. } if output.contains("err")));
    }

    #[test]
    fn test_prompt_done_variants() {
        let mut t = Translator::default();
        t.handle("session/update", &json!({"update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "feito"}}}));
        t.note_denied("Run ls");
        let done = t.handle(PROMPT_DONE, &json!({"result": {"stopReason": "end_turn", "usage": {"inputTokens": 5, "outputTokens": 2}}}));
        assert_eq!(
            done,
            vec![AgentEvent::TurnComplete {
                text: Some("feito".into()),
                is_error: false,
                usage: Some(TokenUsage { input_tokens: 5, output_tokens: 2, cache_read_tokens: 0, cache_creation_tokens: 0 }),
                cost_usd: None,
                duration_ms: None,
                denied_tools: vec!["Run ls".into()],
            }]
        );
        let cancelled = t.handle(PROMPT_DONE, &json!({"result": {"stopReason": "cancelled"}}));
        assert!(matches!(&cancelled[0], AgentEvent::TurnComplete { is_error: true, text: None, .. }));
        let refused = t.handle(PROMPT_DONE, &json!({"result": {"stopReason": "refusal"}}));
        assert!(matches!(&refused[0], AgentEvent::TurnComplete { is_error: true, text: Some(_), .. }));
        let errored = t.handle(PROMPT_DONE, &json!({"error": "quota esgotada"}));
        assert!(matches!(&errored[0], AgentEvent::TurnComplete { is_error: true, text: Some(m), .. } if m == "quota esgotada"));
    }

    #[test]
    fn test_auth_required_detection() {
        assert!(is_auth_required(&RpcError::Remote { code: -32000, message: "Authentication required".into() }));
        assert!(!is_auth_required(&RpcError::Remote { code: -32000, message: "outra coisa".into() }));
        assert!(!is_auth_required(&RpcError::Remote { code: -32602, message: "auth".into() }));
    }
}
