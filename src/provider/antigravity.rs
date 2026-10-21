//! Driver do Antigravity (Google): o CLI oficial `agy`, um processo por turno (ver `agy_session`).
//! O login é do próprio `agy` (feito na primeira execução interativa); o Rusi só confere
//! com `agy models`, que falha sem login.

use async_trait::async_trait;
use std::path::PathBuf;

use super::agy_session;
use super::cli_probe::{default_extra_dirs, probe_version, resolve_binary, run_stdout};
use super::snapshot::{AuthState, ProviderSnapshot};
use super::ProviderDriver;
use crate::session::{AgentSession, SessionConfig, SessionError};

const BINARIES: &[&str] = &["agy", "antigravity"];
const SIGN_IN_HINT: &str = "execute `agy` no terminal e faça login";

pub struct AntigravityDriver {
    binary: Option<PathBuf>,
}

impl AntigravityDriver {
    pub fn new() -> Self {
        Self { binary: None }
    }

    pub fn with_binary(mut self, binary: PathBuf) -> Self {
        self.binary = Some(binary);
        self
    }

    fn find_binary(&self) -> Option<PathBuf> {
        self.binary.clone().or_else(|| resolve_binary(BINARIES, &default_extra_dirs()))
    }
}

impl Default for AntigravityDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// Os ids de `agy models`: uma linha `id<TAB>rótulo` por modelo.
pub fn parse_models(output: &str) -> Vec<String> {
    output.lines().filter_map(|l| l.split('\t').next()).map(str::trim).filter(|id| !id.is_empty()).map(String::from).collect()
}

#[async_trait]
impl ProviderDriver for AntigravityDriver {
    fn id(&self) -> &'static str {
        "antigravity"
    }

    fn display_name(&self) -> &'static str {
        "Antigravity"
    }

    async fn snapshot(&self) -> ProviderSnapshot {
        let Some(binary) = self.find_binary() else {
            return ProviderSnapshot::unavailable(self.id(), self.display_name(), "`agy` não encontrado no PATH");
        };
        let (version, models) = tokio::join!(probe_version(&binary), run_stdout(&binary, &["models"]));
        let auth = if models.is_some() { AuthState::Authenticated { account: None } } else { AuthState::Unauthenticated };
        let message = (auth == AuthState::Unauthenticated).then(|| SIGN_IN_HINT.to_string());
        ProviderSnapshot {
            id: self.id().to_string(),
            display_name: self.display_name().to_string(),
            installed: true,
            version,
            binary: Some(binary),
            auth,
            message,
        }
    }

    async fn models(&self) -> Vec<String> {
        let Some(binary) = self.find_binary() else { return Vec::new() };
        run_stdout(&binary, &["models"]).await.map(|out| parse_models(&out)).unwrap_or_default()
    }

    async fn start_session(&self, config: SessionConfig) -> Result<Box<dyn AgentSession>, SessionError> {
        let binary = self.find_binary().ok_or_else(|| SessionError::NotReady("`agy` não encontrado no PATH".into()))?;
        if self.binary.is_none() && run_stdout(&binary, &["models"]).await.is_none() {
            return Err(SessionError::NotReady(format!("Antigravity sem login: {SIGN_IN_HINT}")));
        }
        Ok(Box::new(agy_session::session(&binary, &config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_models() {
        assert_eq!(parse_models("gemini-4-argon-medium\tGemini 4\nclaude-opus-5-5-high\tOpus\n\n"), ["gemini-4-argon-medium", "claude-opus-5-5-high"]);
        assert!(parse_models("").is_empty());
    }
}
