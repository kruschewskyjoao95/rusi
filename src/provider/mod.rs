//! Drivers de provedores externos. O Rusi não tem modelo nem loop de agente próprios:
//! ele detecta, autentica e hospeda os CLIs/agentes que o usuário já possui.

mod acp_session;
mod antigravity;
pub mod antigravity_auth;
pub mod antigravity_install;
mod claude;
mod claude_session;
mod cursor;
mod cursor_session;
mod codex;
mod codex_session;
mod grok;
mod muse;
mod muse_session;
mod opencode;
mod opencode_session;
mod pi;
mod pi_session;
mod cli_probe;
mod snapshot;

pub use claude::{ClaudeDriver, parse_claude_credentials};
pub use codex::{CodexDriver, parse_account};
pub use muse::MuseDriver;
pub use opencode::OpenCodeDriver;
pub use pi::{PiDriver, parse_auth_check};
pub use antigravity::{AntigravityDriver, harness_for, parse_token_file};
pub use cursor::{CursorDriver, parse_status_output};
pub use grok::{GrokDriver, parse_models_output};
pub use cli_probe::{CliProbeDriver, resolve_binary};
pub use snapshot::{AuthState, ProviderSnapshot};

use async_trait::async_trait;

use crate::session::{AgentSession, SessionConfig, SessionError};

/// Um provedor de agente de código hospedado pelo Rusi.
#[async_trait]
pub trait ProviderDriver: Send + Sync {
    /// Identificador estável (ex: `claude`, `codex`).
    fn id(&self) -> &'static str;

    /// Nome exibido ao usuário.
    fn display_name(&self) -> &'static str;

    /// Estado atual: instalado, versão e autenticação.
    async fn snapshot(&self) -> ProviderSnapshot;

    /// Inicia uma sessão com o agente. Provedores sem suporte ainda devolvem `Unsupported`.
    async fn start_session(&self, _config: SessionConfig) -> Result<Box<dyn AgentSession>, SessionError> {
        Err(SessionError::Unsupported(self.id().to_string()))
    }
}

/// Encontra um driver de primeira parte pelo id.
pub fn driver_by_id(id: &str) -> Option<Box<dyn ProviderDriver>> {
    built_in_drivers().into_iter().find(|d| d.id() == id)
}

/// Drivers de primeira parte, na ordem de exibição.
pub fn built_in_drivers() -> Vec<Box<dyn ProviderDriver>> {
    vec![
        Box::new(ClaudeDriver::new()),
        Box::new(CodexDriver::new()),
        Box::new(AntigravityDriver::new()),
        Box::new(CursorDriver::new()),
        Box::new(MuseDriver::new()),
        Box::new(GrokDriver::new()),
        Box::new(OpenCodeDriver::new()),
        Box::new(PiDriver::new()),
    ]
}

/// Coleta o snapshot de todos os drivers em paralelo, preservando a ordem.
pub async fn snapshot_all(drivers: &[Box<dyn ProviderDriver>]) -> Vec<ProviderSnapshot> {
    futures::future::join_all(drivers.iter().map(|d| d.snapshot())).await
}
