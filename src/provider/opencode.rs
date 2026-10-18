use async_trait::async_trait;
use std::path::PathBuf;

use super::cli_probe::{default_extra_dirs, probe_version, resolve_binary};
use super::opencode_session::OpenCodeSession;
use super::snapshot::{AuthState, ProviderSnapshot};
use super::ProviderDriver;
use crate::session::{AgentSession, SessionConfig, SessionError};

/// Driver do OpenCode: sobe `opencode serve` (v2) e conversa pela API HTTP + SSE.
pub struct OpenCodeDriver {
    binary: Option<PathBuf>,
}

impl OpenCodeDriver {
    pub fn new() -> Self {
        Self { binary: None }
    }

    pub fn with_binary(mut self, binary: PathBuf) -> Self {
        self.binary = Some(binary);
        self
    }

    fn find_binary(&self) -> Option<PathBuf> {
        self.binary.clone().or_else(|| resolve_binary(&["opencode"], &default_extra_dirs()))
    }
}

impl Default for OpenCodeDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ProviderDriver for OpenCodeDriver {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn display_name(&self) -> &'static str {
        "OpenCode"
    }

    async fn snapshot(&self) -> ProviderSnapshot {
        let Some(binary) = self.find_binary() else {
            return ProviderSnapshot::unavailable(self.id(), self.display_name(), "`opencode` não encontrado no PATH");
        };
        ProviderSnapshot {
            id: self.id().to_string(),
            display_name: self.display_name().to_string(),
            installed: true,
            version: probe_version(&binary).await,
            binary: Some(binary),
            // Há modelos gratuitos sem login; as credenciais ficam no servidor do OpenCode.
            auth: AuthState::Unknown,
            message: Some("modelos gratuitos funcionam sem login; veja `opencode auth`".to_string()),
        }
    }

    async fn start_session(&self, config: SessionConfig) -> Result<Box<dyn AgentSession>, SessionError> {
        let binary = self
            .find_binary()
            .ok_or_else(|| SessionError::NotReady("`opencode` não encontrado no PATH".into()))?;
        Ok(Box::new(OpenCodeSession::start(&binary, &config).await?))
    }
}
