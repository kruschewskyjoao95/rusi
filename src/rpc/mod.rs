//! Par JSON-RPC sobre stdio de um processo filho (uma mensagem JSON por linha).
//! Usado pelos provedores que falam JSON-RPC: `codex app-server` (sem campo `jsonrpc`)
//! e agentes ACP (com `"jsonrpc": "2.0"`).

use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};

use crate::tools::process::kill_process_group;

const STDERR_TAIL_LINES: usize = 20;
const CLOSE_GRACE: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Error)]
pub enum RpcError {
    /// O peer respondeu com erro JSON-RPC.
    #[error("{message} (código {code})")]
    Remote { code: i64, message: String },

    /// O processo terminou antes de responder.
    #[error("o processo terminou{}", .0.as_deref().map(|s| format!(": {s}")).unwrap_or_default())]
    Closed(Option<String>),
}

/// Mensagem recebida do peer que não é resposta a um pedido nosso.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Notification { method: String, params: Value },
    /// Pedido do servidor que exige resposta via `respond`/`respond_error`.
    Request { id: Value, method: String, params: Value },
    /// stdout fechou; o processo acabou.
    Exited { stderr: Option<String> },
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>>;

pub struct RpcPeer {
    child: tokio::sync::Mutex<Child>,
    stdin: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    pending: Pending,
    next_id: AtomicI64,
    jsonrpc_field: bool,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    incoming_tx: mpsc::UnboundedSender<Incoming>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Classifica uma linha recebida. `None` se não for JSON-RPC reconhecível.
fn classify(line: &str) -> Option<Classified> {
    let v: Value = serde_json::from_str(line).ok()?;
    let method = v["method"].as_str().map(str::to_string);
    let id = v.get("id").filter(|i| !i.is_null()).cloned();
    match (method, id) {
        (Some(method), Some(id)) => Some(Classified::Request { id, method, params: v["params"].clone() }),
        (Some(method), None) => Some(Classified::Notification { method, params: v["params"].clone() }),
        (None, Some(id)) => {
            let id = id.as_i64()?;
            let outcome = if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
                Err(RpcError::Remote {
                    code: err["code"].as_i64().unwrap_or(0),
                    message: err["message"].as_str().unwrap_or("erro desconhecido").to_string(),
                })
            } else {
                Ok(v["result"].clone())
            };
            Some(Classified::Response { id, outcome })
        }
        (None, None) => None,
    }
}

enum Classified {
    Response { id: i64, outcome: Result<Value, RpcError> },
    Request { id: Value, method: String, params: Value },
    Notification { method: String, params: Value },
}

impl RpcPeer {
    /// Inicia o processo (em grupo próprio) e devolve o peer e o fluxo de mensagens recebidas.
    pub fn spawn(
        mut cmd: Command,
        jsonrpc_field: bool,
    ) -> Result<(Self, mpsc::UnboundedReceiver<Incoming>), std::io::Error> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let pending: Pending = Arc::default();
        let stderr_tail: Arc<Mutex<VecDeque<String>>> = Arc::default();
        let (tx, rx) = mpsc::unbounded_channel();

        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut tail = lock(&tail);
                if tail.len() == STDERR_TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        });

        let injector = tx.clone();
        let reader_pending = pending.clone();
        let reader_tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match classify(&line) {
                    Some(Classified::Response { id, outcome }) => {
                        if let Some(waiter) = lock(&reader_pending).remove(&id) {
                            let _ = waiter.send(outcome);
                        }
                    }
                    Some(Classified::Request { id, method, params }) => {
                        let _ = tx.send(Incoming::Request { id, method, params });
                    }
                    Some(Classified::Notification { method, params }) => {
                        let _ = tx.send(Incoming::Notification { method, params });
                    }
                    None => {}
                }
            }
            let stderr = {
                let tail = lock(&reader_tail);
                (!tail.is_empty()).then(|| tail.iter().cloned().collect::<Vec<_>>().join("\n"))
            };
            for (_, waiter) in lock(&reader_pending).drain() {
                let _ = waiter.send(Err(RpcError::Closed(stderr.clone())));
            }
            let _ = tx.send(Incoming::Exited { stderr });
        });

        let peer = Self {
            child: tokio::sync::Mutex::new(child),
            stdin: Arc::new(tokio::sync::Mutex::new(stdin)),
            pending,
            next_id: AtomicI64::new(1),
            jsonrpc_field,
            stderr_tail,
            incoming_tx: injector,
        };
        Ok((peer, rx))
    }

    /// Enfileira uma mensagem sintética no mesmo fluxo de `Incoming`. Como o canal é FIFO,
    /// ela chega depois de tudo que o processo já emitiu: serve para sinalizar o fim de um
    /// pedido (ex: `session/prompt`) sem ultrapassar as notificações que o precederam.
    pub fn inject(&self, message: Incoming) {
        let _ = self.incoming_tx.send(message);
    }

    pub fn stderr_summary(&self) -> Option<String> {
        let tail = lock(&self.stderr_tail);
        (!tail.is_empty()).then(|| tail.iter().cloned().collect::<Vec<_>>().join("\n"))
    }

    fn envelope(&self, mut message: Value) -> Value {
        if self.jsonrpc_field {
            message["jsonrpc"] = json!("2.0");
        }
        message
    }

    async fn write(&self, message: Value) -> Result<(), RpcError> {
        let mut line = self.envelope(message).to_string();
        line.push('\n');
        let mut guard = self.stdin.lock().await;
        let Some(stdin) = guard.as_mut() else {
            return Err(RpcError::Closed(self.stderr_summary()));
        };
        let result = async {
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await
        }
        .await;
        result.map_err(|_| RpcError::Closed(self.stderr_summary()))
    }

    /// Envia um pedido e espera a resposta.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        if let Err(err) = self.write(json!({ "id": id, "method": method, "params": params })).await {
            lock(&self.pending).remove(&id);
            return Err(err);
        }
        rx.await.unwrap_or_else(|_| Err(RpcError::Closed(self.stderr_summary())))
    }

    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), RpcError> {
        let mut message = json!({ "method": method });
        if let Some(params) = params {
            message["params"] = params;
        }
        self.write(message).await
    }

    /// Responde a um `Incoming::Request`.
    pub async fn respond(&self, id: Value, result: Value) -> Result<(), RpcError> {
        self.write(json!({ "id": id, "result": result })).await
    }

    pub async fn respond_error(&self, id: Value, code: i64, message: &str) -> Result<(), RpcError> {
        self.write(json!({ "id": id, "error": { "code": code, "message": message } })).await
    }

    /// EOF no stdin; se o processo não sair, derruba o grupo inteiro.
    pub async fn close(&self) {
        *self.stdin.lock().await = None;
        let mut child = self.child.lock().await;
        if tokio::time::timeout(CLOSE_GRACE, child.wait()).await.is_err()
            && let Some(pid) = child.id()
        {
            kill_process_group(pid);
            let _ = child.wait().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_variants() {
        assert!(matches!(
            classify(r#"{"id":1,"result":{"a":1}}"#),
            Some(Classified::Response { id: 1, outcome: Ok(_) })
        ));
        assert!(matches!(
            classify(r#"{"id":2,"error":{"code":-32600,"message":"ruim"}}"#),
            Some(Classified::Response { id: 2, outcome: Err(RpcError::Remote { code: -32600, .. }) })
        ));
        assert!(matches!(classify(r#"{"method":"x","params":{}}"#), Some(Classified::Notification { .. })));
        assert!(matches!(
            classify(r#"{"id":"abc","method":"item/requestApproval","params":{}}"#),
            Some(Classified::Request { .. })
        ));
        assert!(classify("lixo").is_none());
        assert!(classify("{}").is_none());
    }

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", script]);
        cmd
    }

    #[tokio::test]
    async fn test_request_response_roundtrip() {
        // Eco: devolve o id recebido.
        let script = r#"read -r line; id=${line#*\"id\":}; id=${id%%,*}; echo "{\"id\":$id,\"result\":{\"ok\":true}}"; sleep 1"#;
        let (peer, _rx) = RpcPeer::spawn(sh(script), false).unwrap();
        let result = peer.request("ping", json!({})).await.unwrap();
        assert_eq!(result["ok"], true);
    }

    #[tokio::test]
    async fn test_pending_requests_fail_when_process_exits() {
        let (peer, mut rx) = RpcPeer::spawn(sh("echo boom >&2; exit 2"), false).unwrap();
        let err = peer.request("ping", json!({})).await.unwrap_err();
        assert!(matches!(err, RpcError::Closed(_)));
        match rx.recv().await {
            Some(Incoming::Exited { stderr }) => assert_eq!(stderr.as_deref(), Some("boom")),
            other => panic!("esperava Exited, veio {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_server_requests_and_notifications_are_forwarded() {
        let script = r#"echo '{"method":"n","params":{"a":1}}'; echo '{"id":9,"method":"ask","params":{}}'; read -r reply; echo "$reply" >&2; sleep 1"#;
        let (peer, mut rx) = RpcPeer::spawn(sh(script), true).unwrap();
        assert!(matches!(rx.recv().await, Some(Incoming::Notification { method, .. }) if method == "n"));
        let Some(Incoming::Request { id, .. }) = rx.recv().await else { panic!("esperava Request") };
        peer.respond(id, json!({"decision":"accept"})).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let echoed = peer.stderr_summary().unwrap();
        assert!(echoed.contains(r#""jsonrpc":"2.0""#) && echoed.contains("accept"), "got {echoed}");
    }
}
