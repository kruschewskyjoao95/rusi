//! Sessão com o servidor v2 do OpenCode: `opencode serve` com senha gerada (Basic auth),
//! API REST em `/api/session/*` e eventos por SSE em `/api/event`.

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{Value, json};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex as AsyncMutex, mpsc};

use crate::config::AgentMode;
use crate::session::{AgentEvent, AgentSession, ApprovalRequest, PendingApprovals, SessionConfig, SessionError, TokenUsage};
use crate::tools::process::kill_process_group;

const USERNAME: &str = "opencode";
const SERVER_START_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CLOSE_GRACE: Duration = Duration::from_secs(3);

/// Regras de permissão da sessão; a última que casa vence.
pub(super) fn session_rules(config: &SessionConfig) -> Vec<Value> {
    let rule = |action: &str, effect: &str| json!({ "action": action, "resource": "*", "effect": effect });
    if config.dangerously_skip_permissions {
        return vec![rule("*", "allow")];
    }
    let edit = if config.mode == AgentMode::Auto { "allow" } else { "ask" };
    let mut rules = vec![rule("shell", "ask"), rule("edit", edit), rule("external_directory", "ask")];
    if config.mode == AgentMode::Plan {
        rules.push(rule("edit", "deny"));
    }
    rules
}

pub(super) fn auto_accept(config: &SessionConfig) -> bool {
    config.dangerously_skip_permissions || config.mode == AgentMode::Auto
}

/// Em `accept-edits` as regras acima mandam perguntar; quem decide é o usuário.
pub(super) fn ask_user(config: &SessionConfig) -> bool {
    config.mode == AgentMode::AcceptEdits && !config.dangerously_skip_permissions
}

async fn reply_permission(http: &reqwest::Client, base: &str, password: &str, sid: &str, request_id: &str, approved: bool) {
    let decision = if approved { "once" } else { "reject" };
    let _ = http
        .post(format!("{base}/api/session/{sid}/permission/{request_id}/reply"))
        .basic_auth(USERNAME, Some(password))
        .json(&json!({ "decision": decision }))
        .send()
        .await;
}

/// `provider/modelo#variante` → corpo `model` da API.
pub(super) fn parse_model(spec: &str) -> Option<Value> {
    let (provider, rest) = spec.split_once('/')?;
    let (id, variant) = match rest.split_once('#') {
        Some((id, variant)) => (id, Some(variant)),
        None => (rest, None),
    };
    if provider.is_empty() || id.is_empty() {
        return None;
    }
    let mut model = json!({ "id": id, "providerID": provider });
    if let Some(variant) = variant.filter(|v| !v.is_empty()) {
        model["variant"] = json!(variant);
    }
    Some(model)
}

/// Extrai a URL de "server listening on http://127.0.0.1:PORT".
pub(super) fn parse_listening_url(line: &str) -> Option<String> {
    let lower = line.to_lowercase();
    let at = lower.find("server listening on")? + "server listening on".len();
    line[at..].split_whitespace().next().filter(|u| u.starts_with("http")).map(str::to_string)
}

/// Separa um fluxo SSE em campos `data:`, um por evento.
#[derive(Default)]
pub(super) struct SseFramer {
    buffer: String,
}

impl SseFramer {
    pub(super) fn push(&mut self, chunk: &str) -> Vec<String> {
        self.buffer.push_str(&chunk.replace("\r\n", "\n"));
        let mut out = Vec::new();
        while let Some(end) = self.buffer.find("\n\n") {
            let frame: String = self.buffer.drain(..end + 2).collect();
            let data: Vec<&str> = frame
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(|d| d.strip_prefix(' ').unwrap_or(d))
                .collect();
            if !data.is_empty() {
                out.push(data.join("\n"));
            }
        }
        out
    }
}

fn text_of(content: &Value) -> String {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("")
}

#[derive(Debug, Default)]
pub(super) struct Translator {
    session_id: String,
    text: String,
    usage: Option<TokenUsage>,
    cost: f64,
    saw_cost: bool,
    denied: Vec<String>,
    tool_names: std::collections::HashMap<String, String>,
}

/// O que o tradutor pede ao driver além dos eventos.
#[derive(Debug, PartialEq)]
pub(super) enum Action {
    Event(AgentEvent),
    /// Responder a um pedido de permissão: (requestID, ação, recursos pedidos).
    Permission { request_id: String, action: String, resources: Value },
}

impl Translator {
    pub(super) fn new(session_id: &str) -> Self {
        Self { session_id: session_id.to_string(), ..Default::default() }
    }

    pub(super) fn note_denied(&mut self, tool: &str) {
        self.denied.push(tool.to_string());
    }

    pub(super) fn handle(&mut self, event: &Value) -> Vec<Action> {
        let data = &event["data"];
        let for_us = data["sessionID"].as_str() == Some(self.session_id.as_str());
        if !for_us {
            return Vec::new();
        }
        let ev = |e: AgentEvent| vec![Action::Event(e)];
        match event["type"].as_str() {
            Some("session.text.delta") => {
                let delta = data["delta"].as_str().unwrap_or("");
                self.text.push_str(delta);
                if delta.is_empty() { Vec::new() } else { ev(AgentEvent::TextDelta(delta.to_string())) }
            }
            Some("session.reasoning.delta") => match data["delta"].as_str() {
                Some(d) if !d.is_empty() => ev(AgentEvent::ThinkingDelta(d.to_string())),
                _ => Vec::new(),
            },
            Some("session.tool.input.started") => {
                if let (Some(id), Some(name)) = (data["id"].as_str(), data["name"].as_str()) {
                    self.tool_names.insert(id.to_string(), name.to_string());
                }
                Vec::new()
            }
            Some("session.tool.called") => match data["id"].as_str() {
                Some(id) => ev(AgentEvent::ToolUse {
                    id: id.to_string(),
                    name: self
                        .tool_names
                        .get(id)
                        .cloned()
                        .or_else(|| data["name"].as_str().map(str::to_string))
                        .unwrap_or_else(|| "tool".to_string()),
                    input: data["input"].clone(),
                }),
                None => Vec::new(),
            },
            Some("session.tool.success") => match data["id"].as_str() {
                Some(id) => ev(AgentEvent::ToolResult { id: id.to_string(), output: text_of(&data["content"]), is_error: false }),
                None => Vec::new(),
            },
            Some("session.tool.failed") => match data["id"].as_str() {
                Some(id) => ev(AgentEvent::ToolResult {
                    id: id.to_string(),
                    output: data["error"]["message"].as_str().unwrap_or("falhou").to_string(),
                    is_error: true,
                }),
                None => Vec::new(),
            },
            Some("session.step.ended") => {
                self.add_usage(data);
                Vec::new()
            }
            Some("permission.asked") => match (data["id"].as_str(), data["action"].as_str()) {
                (Some(id), Some(action)) => {
                    vec![Action::Permission { request_id: id.to_string(), action: action.to_string(), resources: data["resources"].clone() }]
                }
                _ => Vec::new(),
            },
            Some("session.execution.succeeded") => ev(self.finish(false, None)),
            Some("session.execution.interrupted") => ev(self.finish(true, None)),
            Some("session.execution.failed") => {
                let message = data["error"]["message"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| data["error"].as_str().map(str::to_string))
                    .unwrap_or_else(|| "a execução falhou".to_string());
                ev(self.finish(true, Some(message)))
            }
            _ => Vec::new(),
        }
    }

    fn add_usage(&mut self, data: &Value) {
        let tokens = &data["tokens"];
        if !tokens.is_object() {
            return;
        }
        let n = |v: &Value| v.as_u64().unwrap_or(0);
        let usage = self.usage.get_or_insert_with(TokenUsage::default);
        usage.input_tokens += n(&tokens["input"]);
        usage.output_tokens += n(&tokens["output"]);
        usage.cache_read_tokens += n(&tokens["cache"]["read"]);
        usage.cache_creation_tokens += n(&tokens["cache"]["write"]);
        if let Some(cost) = data["cost"].as_f64() {
            self.cost += cost;
            self.saw_cost = true;
        }
    }

    fn finish(&mut self, is_error: bool, error: Option<String>) -> AgentEvent {
        let text = std::mem::take(&mut self.text);
        let cost = std::mem::take(&mut self.cost);
        let denied_tools = std::mem::take(&mut self.denied);
        // Recusar uma permissão encerra a execução sem mensagem; diz o porquê.
        let error = error.or_else(|| {
            (is_error && text.is_empty() && !denied_tools.is_empty())
                .then(|| format!("permissão negada: {}", denied_tools.join(", ")))
        });
        AgentEvent::TurnComplete {
            text: error.or(Some(text).filter(|t| !t.is_empty())),
            is_error,
            usage: self.usage.take(),
            cost_usd: std::mem::take(&mut self.saw_cost).then_some(cost),
            duration_ms: None,
            denied_tools,
        }
    }
}

pub struct OpenCodeSession {
    http: reqwest::Client,
    base: String,
    password: String,
    session_id: String,
    child: Arc<AsyncMutex<Child>>,
    events: mpsc::UnboundedReceiver<AgentEvent>,
    approvals: PendingApprovals,
}

fn random_password() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

impl OpenCodeSession {
    pub(super) async fn start(binary: &Path, config: &SessionConfig) -> Result<Self, SessionError> {
        let password = random_password();
        let mut cmd = Command::new(binary);
        cmd.args(["serve", "--hostname", "127.0.0.1", "--port", "0"])
            .env("OPENCODE_PASSWORD", &password)
            .env_remove("OPENCODE_SERVER_PASSWORD")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        if std::env::var_os("OPENCODE_CONFIG_CONTENT").is_none() {
            cmd.env("OPENCODE_CONFIG_CONTENT", "{}");
        }
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }
        let mut child = cmd.spawn().map_err(SessionError::Spawn)?;

        // O servidor anuncia a URL numa linha de log; lê stdout e stderr.
        let (url_tx, mut url_rx) = mpsc::unbounded_channel::<String>();
        for reader in [
            child.stdout.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
            child.stderr.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let tx = url_tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(reader).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(url) = parse_listening_url(&line) {
                        let _ = tx.send(url);
                    }
                }
            });
        }
        drop(url_tx);

        let child = Arc::new(AsyncMutex::new(child));
        let base = match tokio::time::timeout(SERVER_START_TIMEOUT, url_rx.recv()).await {
            Ok(Some(url)) => url.trim_end_matches('/').to_string(),
            _ => {
                Self::kill(&child).await;
                return Err(SessionError::NotReady("o servidor do OpenCode não iniciou".into()));
            }
        };

        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| SessionError::NotReady(format!("cliente HTTP: {e}")))?;
        let mut session = Self {
            http,
            base,
            password,
            session_id: String::new(),
            child,
            events: mpsc::unbounded_channel().1,
            approvals: PendingApprovals::default(),
        };
        match session.open(config).await {
            Ok(()) => Ok(session),
            Err(err) => {
                session.close().await;
                Err(err)
            }
        }
    }

    async fn kill(child: &AsyncMutex<Child>) {
        let mut child = child.lock().await;
        if let Some(pid) = child.id() {
            kill_process_group(pid);
        }
        let _ = child.wait().await;
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{}", self.base, path))
            .basic_auth(USERNAME, Some(&self.password))
    }

    async fn call(&self, req: reqwest::RequestBuilder) -> Result<Value, SessionError> {
        let response = req.send().await.map_err(|e| SessionError::NotReady(format!("OpenCode: {e}")))?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let detail = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| v["message"].as_str().map(str::to_string))
                .unwrap_or_else(|| body.chars().take(200).collect());
            return Err(SessionError::NotReady(format!("OpenCode respondeu HTTP {status}: {detail}")));
        }
        Ok(serde_json::from_str(&body).unwrap_or(Value::Null))
    }

    /// Assina `/api/event`, cria (ou retoma) a sessão e liga o tradutor.
    async fn open(&mut self, config: &SessionConfig) -> Result<(), SessionError> {
        let info = self.call(self.request(reqwest::Method::GET, "/api/info")).await?;
        let _version = info["version"].as_str().unwrap_or("?");

        // O SSE precisa estar ativo antes do primeiro prompt, senão se perdem eventos.
        let stream = self
            .http
            .get(format!("{}/api/event", self.base))
            .basic_auth(USERNAME, Some(&self.password))
            .header("accept", "text/event-stream")
            .timeout(Duration::from_secs(60 * 60 * 24))
            .send()
            .await
            .map_err(|e| SessionError::NotReady(format!("OpenCode (eventos): {e}")))?;
        if !stream.status().is_success() {
            return Err(SessionError::NotReady(format!("OpenCode (eventos) respondeu HTTP {}", stream.status())));
        }

        let rules = session_rules(config);
        let directory = config
            .cwd
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .map(|p| p.display().to_string());

        let (session_id, model) = match &config.resume {
            Some(id) => {
                let found = self.call(self.request(reqwest::Method::GET, &format!("/api/session/{id}"))).await?;
                self.call(
                    self.request(reqwest::Method::PATCH, &format!("/api/session/{id}"))
                        .json(&json!({ "permissions": rules })),
                )
                .await?;
                (id.clone(), found["data"]["model"]["id"].as_str().map(str::to_string))
            }
            None => {
                let mut body = json!({ "permissions": rules });
                if let Some(dir) = directory {
                    body["location"] = json!({ "directory": dir });
                }
                if let Some(model) = config.model.as_deref().and_then(parse_model) {
                    body["model"] = model;
                }
                let created = self.call(self.request(reqwest::Method::POST, "/api/session").json(&body)).await?;
                let id = created["data"]["id"]
                    .as_str()
                    .ok_or_else(|| SessionError::NotReady("o OpenCode não devolveu o id da sessão".into()))?
                    .to_string();
                (id, config.model.clone())
            }
        };
        self.session_id = session_id.clone();

        let (tx, events) = mpsc::unbounded_channel();
        let _ = tx.send(AgentEvent::SessionStarted { session_id: session_id.clone(), model });
        self.events = events;

        let accept = auto_accept(config);
        let ask = ask_user(config);
        let task_approvals = self.approvals.clone();
        let http = self.http.clone();
        let base = self.base.clone();
        let password = self.password.clone();
        let sid = session_id;
        tokio::spawn(async move {
            let mut translator = Translator::new(&sid);
            let mut framer = SseFramer::default();
            let mut bytes = stream.bytes_stream();
            while let Some(chunk) = bytes.next().await {
                let Ok(chunk) = chunk else { break };
                for data in framer.push(&String::from_utf8_lossy(&chunk)) {
                    let Ok(event) = serde_json::from_str::<Value>(&data) else { continue };
                    for action in translator.handle(&event) {
                        match action {
                            Action::Event(e) => {
                                if tx.send(e).is_err() {
                                    return;
                                }
                            }
                            Action::Permission { request_id, action, resources } => {
                                if ask {
                                    let (http, base, password, sid) = (http.clone(), base.clone(), password.clone(), sid.clone());
                                    let id = request_id.clone();
                                    task_approvals.register(request_id.clone(), move |approved| async move {
                                        reply_permission(&http, &base, &password, &sid, &id, approved).await;
                                    });
                                    let request = ApprovalRequest { id: request_id, tool: action, input: resources };
                                    if tx.send(AgentEvent::ApprovalRequested(request)).is_err() {
                                        return;
                                    }
                                    continue;
                                }
                                if !accept {
                                    translator.note_denied(&action);
                                }
                                reply_permission(&http, &base, &password, &sid, &request_id, accept).await;
                            }
                        }
                    }
                }
            }
            let _ = tx.send(AgentEvent::Error("a conexão de eventos do OpenCode foi encerrada".into()));
        });
        Ok(())
    }
}

#[async_trait]
impl AgentSession for OpenCodeSession {
    async fn send(&mut self, prompt: &str) -> Result<(), SessionError> {
        self.call(
            self.request(reqwest::Method::POST, &format!("/api/session/{}/prompt", self.session_id))
                .json(&json!({ "text": prompt })),
        )
        .await
        .map(|_| ())
    }

    async fn next_event(&mut self) -> Option<AgentEvent> {
        self.events.recv().await
    }

    async fn respond_approval(&mut self, id: &str, approved: bool) -> Result<(), SessionError> {
        self.approvals.resolve(id, approved).await
    }

    async fn interrupt(&mut self) -> Result<(), SessionError> {
        self.call(self.request(reqwest::Method::POST, &format!("/api/session/{}/interrupt", self.session_id)))
            .await
            .map(|_| ())
    }

    async fn close(&mut self) {
        let wait = async { self.child.lock().await.wait().await };
        // Não há EOF útil para o servidor: derruba o grupo de processos.
        if let Some(pid) = self.child.lock().await.id() {
            kill_process_group(pid);
        }
        let _ = tokio::time::timeout(CLOSE_GRACE, wait).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(mode: AgentMode, skip: bool) -> SessionConfig {
        SessionConfig { mode, dangerously_skip_permissions: skip, ..Default::default() }
    }

    #[test]
    fn test_rules_by_mode() {
        let effect = |rules: &[Value], action: &str| {
            rules.iter().rev().find(|r| r["action"] == action).map(|r| r["effect"].as_str().unwrap().to_string())
        };
        let accept = session_rules(&cfg(AgentMode::AcceptEdits, false));
        assert_eq!((effect(&accept, "shell").as_deref(), effect(&accept, "edit").as_deref()), (Some("ask"), Some("ask")));
        let auto = session_rules(&cfg(AgentMode::Auto, false));
        assert_eq!(effect(&auto, "edit").as_deref(), Some("allow"));
        let plan = session_rules(&cfg(AgentMode::Plan, false));
        assert_eq!(effect(&plan, "edit").as_deref(), Some("deny"));
        let skip = session_rules(&cfg(AgentMode::Plan, true));
        assert_eq!(skip, vec![json!({"action": "*", "resource": "*", "effect": "allow"})]);
        assert!(auto_accept(&cfg(AgentMode::Auto, false)) && !auto_accept(&cfg(AgentMode::Plan, false)));
        assert!(ask_user(&cfg(AgentMode::AcceptEdits, false)));
        assert!(!ask_user(&cfg(AgentMode::Plan, false)) && !ask_user(&cfg(AgentMode::AcceptEdits, true)));
    }

    #[test]
    fn test_parse_model_and_listening_url() {
        assert_eq!(parse_model("opencode/big-pickle"), Some(json!({"id": "big-pickle", "providerID": "opencode"})));
        assert_eq!(
            parse_model("anthropic/claude-x#high"),
            Some(json!({"id": "claude-x", "providerID": "anthropic", "variant": "high"}))
        );
        assert_eq!(parse_model("sem-barra"), None);
        assert_eq!(
            parse_listening_url("server listening on http://127.0.0.1:32783"),
            Some("http://127.0.0.1:32783".to_string())
        );
        assert_eq!(parse_listening_url("outra linha"), None);
    }

    #[test]
    fn test_sse_framer_handles_split_chunks_and_comments() {
        let mut f = SseFramer::default();
        assert!(f.push("data: {\"a\"").is_empty());
        assert_eq!(f.push(":1}\n\n: heartbeat\n\ndata: {\"b\":2}\n\n"), vec!["{\"a\":1}".to_string(), "{\"b\":2}".to_string()]);
        assert_eq!(f.push("id: 7\r\ndata: x\r\n\r\n"), vec!["x".to_string()]);
    }

    #[test]
    fn test_translates_a_full_turn_and_ignores_other_sessions() {
        let mut t = Translator::new("ses_1");
        let one = |ev: Value, t: &mut Translator| t.handle(&ev);
        assert!(one(json!({"type": "session.text.delta", "data": {"sessionID": "ses_other", "delta": "x"}}), &mut t).is_empty());
        assert_eq!(
            one(json!({"type": "session.text.delta", "data": {"sessionID": "ses_1", "delta": "pong"}}), &mut t),
            vec![Action::Event(AgentEvent::TextDelta("pong".into()))]
        );
        assert_eq!(
            one(json!({"type": "session.tool.called", "data": {"sessionID": "ses_1", "id": "c1", "name": "shell", "input": {"command": "ls"}}}), &mut t),
            vec![Action::Event(AgentEvent::ToolUse { id: "c1".into(), name: "shell".into(), input: json!({"command": "ls"}) })]
        );
        assert_eq!(
            one(json!({"type": "session.tool.success", "data": {"sessionID": "ses_1", "id": "c1", "content": [{"type": "text", "text": "a.rs"}]}}), &mut t),
            vec![Action::Event(AgentEvent::ToolResult { id: "c1".into(), output: "a.rs".into(), is_error: false })]
        );
        one(json!({"type": "session.step.ended", "data": {"sessionID": "ses_1", "cost": 0.5, "tokens": {"input": 10, "output": 2, "cache": {"read": 3, "write": 1}}}}), &mut t);
        t.note_denied("shell");
        let done = one(json!({"type": "session.execution.succeeded", "data": {"sessionID": "ses_1"}}), &mut t);
        assert_eq!(
            done,
            vec![Action::Event(AgentEvent::TurnComplete {
                text: Some("pong".into()),
                is_error: false,
                usage: Some(TokenUsage { input_tokens: 10, output_tokens: 2, cache_read_tokens: 3, cache_creation_tokens: 1 }),
                cost_usd: Some(0.5),
                duration_ms: None,
                denied_tools: vec!["shell".into()],
            })]
        );
    }

    #[test]
    fn test_tool_name_comes_from_input_started() {
        let mut t = Translator::new("s");
        t.handle(&json!({"type": "session.tool.input.started", "data": {"sessionID": "s", "id": "c1", "name": "shell"}}));
        let called = t.handle(&json!({"type": "session.tool.called", "data": {"sessionID": "s", "id": "c1", "input": {"command": "ls"}}}));
        assert!(matches!(&called[0], Action::Event(AgentEvent::ToolUse { name, .. }) if name == "shell"));
    }

    #[test]
    fn test_denied_permission_explains_the_interrupted_turn() {
        let mut t = Translator::new("s");
        t.note_denied("shell");
        let done = t.handle(&json!({"type": "session.execution.interrupted", "data": {"sessionID": "s"}}));
        assert!(matches!(&done[0], Action::Event(AgentEvent::TurnComplete { text: Some(m), is_error: true, .. }) if m == "permissão negada: shell"));
    }

    #[test]
    fn test_failure_interrupt_and_permission() {
        let mut t = Translator::new("s");
        let failed = t.handle(&json!({"type": "session.execution.failed", "data": {"sessionID": "s", "error": {"message": "sem modelo"}}}));
        assert!(matches!(&failed[0], Action::Event(AgentEvent::TurnComplete { is_error: true, text: Some(m), .. }) if m == "sem modelo"));
        let interrupted = t.handle(&json!({"type": "session.execution.interrupted", "data": {"sessionID": "s", "reason": "user"}}));
        assert!(matches!(&interrupted[0], Action::Event(AgentEvent::TurnComplete { is_error: true, .. })));
        let failed_tool = t.handle(&json!({"type": "session.tool.failed", "data": {"sessionID": "s", "id": "c", "error": {"message": "boom"}}}));
        assert_eq!(failed_tool, vec![Action::Event(AgentEvent::ToolResult { id: "c".into(), output: "boom".into(), is_error: true })]);
        let ask = t.handle(&json!({"type": "permission.asked", "data": {"id": "perm_1", "sessionID": "s", "action": "shell", "resources": ["ls"]}}));
        assert_eq!(ask, vec![Action::Permission { request_id: "perm_1".into(), action: "shell".into(), resources: json!(["ls"]) }]);
    }
}
