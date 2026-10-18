use async_trait::async_trait;
use std::path::PathBuf;

use super::cli_probe::{default_extra_dirs, probe_version, resolve_binary};
use super::snapshot::{AuthState, ProviderSnapshot};
use super::ProviderDriver;
use super::claude_session::ClaudeSession;
use crate::session::{AgentSession, SessionConfig, SessionError};

/// Driver do Claude: reutiliza o CLI `claude` e o login (Pro/Max) já existente.
/// O Rusi lê o arquivo de credenciais só para saber se há login; o token nunca
/// sai deste módulo.
pub struct ClaudeDriver {
    config_dir: Option<PathBuf>,
    /// Binário fixo em vez de procurar `claude` no PATH (usado em testes).
    binary: Option<PathBuf>,
}

impl ClaudeDriver {
    pub fn new() -> Self {
        let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".claude")));
        Self { config_dir, binary: None }
    }

    pub fn with_binary(mut self, binary: PathBuf) -> Self {
        self.binary = Some(binary);
        self
    }

    fn find_binary(&self) -> Option<PathBuf> {
        self.binary
            .clone()
            .or_else(|| resolve_binary(&["claude"], &default_extra_dirs()))
    }

    fn read_auth(&self) -> AuthState {
        let Some(dir) = &self.config_dir else {
            return AuthState::Unknown;
        };
        match std::fs::read_to_string(dir.join(".credentials.json")) {
            Ok(raw) => parse_claude_credentials(&raw),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => AuthState::Unauthenticated,
            Err(_) => AuthState::Unknown,
        }
    }
}

impl Default for ClaudeDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// Interpreta `.credentials.json` sem expor o token. Considera autenticado quando
/// há `claudeAiOauth.accessToken`; o plano (`subscriptionType`) vira o identificador da conta.
pub fn parse_claude_credentials(raw: &str) -> AuthState {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return AuthState::Unknown;
    };
    let oauth = &value["claudeAiOauth"];
    let has_token = oauth["accessToken"].as_str().is_some_and(|t| !t.is_empty());
    if !has_token {
        return AuthState::Unauthenticated;
    }
    AuthState::Authenticated {
        account: oauth["subscriptionType"].as_str().map(str::to_string),
    }
}

#[async_trait]
impl ProviderDriver for ClaudeDriver {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn display_name(&self) -> &'static str {
        "Claude (Claude Code)"
    }

    async fn snapshot(&self) -> ProviderSnapshot {
        let Some(binary) = self.find_binary() else {
            return ProviderSnapshot::unavailable(
                self.id(),
                self.display_name(),
                "`claude` não encontrado; instale o Claude Code",
            );
        };
        let auth = self.read_auth();
        let message = match auth {
            AuthState::Unauthenticated => Some("execute `claude` e faça login".to_string()),
            _ => None,
        };
        ProviderSnapshot {
            id: self.id().to_string(),
            display_name: self.display_name().to_string(),
            installed: true,
            version: probe_version(&binary).await,
            binary: Some(binary),
            auth,
            message,
        }
    }

    async fn start_session(&self, config: SessionConfig) -> Result<Box<dyn AgentSession>, SessionError> {
        let binary = self.find_binary().ok_or_else(|| {
            SessionError::NotReady("`claude` não encontrado; instale o Claude Code".into())
        })?;
        if self.binary.is_none() && self.read_auth() == AuthState::Unauthenticated {
            return Err(SessionError::NotReady(
                "Claude sem login: execute `claude` e faça login".into(),
            ));
        }
        Ok(Box::new(ClaudeSession::spawn(&binary, &config)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credentials_with_token_is_authenticated() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"sk-secret","subscriptionType":"max"}}"#;
        assert_eq!(
            parse_claude_credentials(raw),
            AuthState::Authenticated { account: Some("max".to_string()) }
        );
    }

    #[test]
    fn test_credentials_without_token_is_unauthenticated() {
        assert_eq!(parse_claude_credentials("{}"), AuthState::Unauthenticated);
        assert_eq!(
            parse_claude_credentials(r#"{"claudeAiOauth":{"accessToken":""}}"#),
            AuthState::Unauthenticated
        );
    }

    #[test]
    fn test_invalid_json_is_unknown() {
        assert_eq!(parse_claude_credentials("not json"), AuthState::Unknown);
    }
}
