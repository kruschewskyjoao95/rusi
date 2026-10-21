//! Sessões com agentes externos. Cada provedor traduz o protocolo do seu CLI para
//! `AgentEvent`, de modo que o resto do Rusi não depende de nenhum provedor específico.

mod approvals;
mod runner;

pub use approvals::PendingApprovals;

pub use runner::{TurnSummary, deny_all, run_turn};

use async_trait::async_trait;
use std::path::PathBuf;
use thiserror::Error;

use crate::config::{AgentMode, EffortLevel};

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("O provedor '{0}' ainda não suporta sessões no Rusi")]
    Unsupported(String),

    #[error("{0}")]
    NotReady(String),

    #[error("Falha ao iniciar o processo do agente: {0}")]
    Spawn(std::io::Error),

    #[error("Erro de I/O com o agente: {0}")]
    Io(#[from] std::io::Error),

    #[error("A sessão do agente já foi encerrada{}", .0.as_deref().map(|s| format!(": {s}")).unwrap_or_default())]
    Closed(Option<String>),

    #[error(transparent)]
    Db(#[from] crate::persistence::DbError),
}

/// Parâmetros para iniciar uma sessão.
#[derive(Debug, Clone, Default)]
pub struct SessionConfig {
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    pub effort: EffortLevel,
    pub mode: AgentMode,
    pub dangerously_skip_permissions: bool,
    /// Id da sessão do próprio provedor, para retomar uma conversa.
    pub resume: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

/// Pedido de permissão do agente para usar uma ferramenta.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ApprovalRequest {
    /// Id do pedido, devolvido em `AgentSession::respond_approval`.
    pub id: String,
    pub tool: String,
    pub input: serde_json::Value,
}

/// Evento normalizado emitido por qualquer provedor.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AgentEvent {
    /// A sessão do provedor existe; guarde `session_id` para retomar depois.
    SessionStarted { session_id: String, model: Option<String> },
    TextDelta(String),
    ThinkingDelta(String),
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { id: String, output: String, is_error: bool },
    /// O agente aguarda uma resposta via `AgentSession::respond_approval`.
    ApprovalRequested(ApprovalRequest),
    /// Fim do turno.
    TurnComplete {
        text: Option<String>,
        is_error: bool,
        usage: Option<TokenUsage>,
        cost_usd: Option<f64>,
        duration_ms: Option<u64>,
        /// Ferramentas que o agente tentou usar e foram negadas por permissão.
        denied_tools: Vec<String>,
    },
    /// Falha fatal da sessão (ex: processo morreu).
    Error(String),
}

/// Uma conversa viva com um agente.
#[async_trait]
pub trait AgentSession: Send {
    /// Envia um prompt do usuário, iniciando um turno.
    async fn send(&mut self, prompt: &str) -> Result<(), SessionError>;

    /// Próximo evento; `None` quando a sessão terminou.
    async fn next_event(&mut self) -> Option<AgentEvent>;

    /// Responde a um `AgentEvent::ApprovalRequested`.
    async fn respond_approval(&mut self, _id: &str, _approved: bool) -> Result<(), SessionError> {
        Err(SessionError::NotReady("este provedor não suporta aprovações interativas".into()))
    }

    /// Pede ao agente para parar o turno atual, mantendo a sessão aberta.
    async fn interrupt(&mut self) -> Result<(), SessionError>;

    /// Encerra o agente e todos os processos filhos dele.
    async fn close(&mut self);
}

#[cfg(test)]
mod serde_tests {
    use super::*;

    #[test]
    fn test_events_serialize_with_type_and_data() {
        let json = |e: AgentEvent| serde_json::to_value(e).unwrap();
        assert_eq!(json(AgentEvent::TextDelta("oi".into())), serde_json::json!({"type": "text_delta", "data": "oi"}));
        let ask = json(AgentEvent::ApprovalRequested(ApprovalRequest { id: "1".into(), tool: "Bash".into(), input: serde_json::json!({}) }));
        assert_eq!(ask["type"], "approval_requested");
        assert_eq!(ask["data"]["tool"], "Bash");
    }
}
