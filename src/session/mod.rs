//! Sessões com agentes externos. Cada provedor traduz o protocolo do seu CLI para
//! `AgentEvent`, de modo que o resto do Rusi não depende de nenhum provedor específico.

mod runner;

pub use runner::{TurnSummary, run_turn};

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

/// Evento normalizado emitido por qualquer provedor.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// A sessão do provedor existe; guarde `session_id` para retomar depois.
    SessionStarted { session_id: String, model: Option<String> },
    TextDelta(String),
    ThinkingDelta(String),
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { id: String, output: String, is_error: bool },
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

    /// Pede ao agente para parar o turno atual, mantendo a sessão aberta.
    async fn interrupt(&mut self) -> Result<(), SessionError>;

    /// Encerra o agente e todos os processos filhos dele.
    async fn close(&mut self);
}
