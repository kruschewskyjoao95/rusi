use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use super::cli_probe::{default_extra_dirs, probe_version, resolve_binary};
use super::pi_session::PiSession;
use super::snapshot::{AuthState, ProviderSnapshot};
use super::ProviderDriver;
use crate::session::{AgentSession, SessionConfig, SessionError};

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Driver do Pi: usa o CLI `pi` (`pi --mode rpc`) com a configuração e o login do próprio Pi.
pub struct PiDriver {
    binary: Option<PathBuf>,
}

impl PiDriver {
    pub fn new() -> Self {
        Self { binary: None }
    }

    pub fn with_binary(mut self, binary: PathBuf) -> Self {
        self.binary = Some(binary);
        self
    }

    fn find_binary(&self) -> Option<PathBuf> {
        self.binary.clone().or_else(|| resolve_binary(&["pi"], &default_extra_dirs()))
    }

    /// Provedor e modelo ativos, lidos de `get_state` num processo RPC descartável.
    async fn active_model(&self, binary: &PathBuf) -> Option<(String, String)> {
        let probe = async {
            let mut child = Command::new(binary)
                .args(["--mode", "rpc", "--no-session"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .ok()?;
            let mut stdin = child.stdin.take()?;
            stdin.write_all(format!("{}\n", json!({ "type": "get_state", "id": "probe" })).as_bytes()).await.ok()?;
            let mut lines = BufReader::new(child.stdout.take()?).lines();
            while let Some(line) = lines.next_line().await.ok()? {
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if v["type"] == "response" && v["id"] == "probe" {
                    let model = &v["data"]["model"];
                    return Some((model["provider"].as_str()?.to_string(), model["id"].as_str()?.to_string()));
                }
            }
            None
        };
        tokio::time::timeout(PROBE_TIMEOUT, probe).await.ok().flatten()
    }

    async fn auth_for(&self, binary: &PathBuf, provider: &str) -> AuthState {
        let run = Command::new(binary)
            .args(["auth", "check", "--provider", provider, "--json", "--no-refresh"])
            .kill_on_drop(true)
            .output();
        let Ok(Ok(output)) = tokio::time::timeout(PROBE_TIMEOUT, run).await else {
            return AuthState::Unknown;
        };
        parse_auth_check(&String::from_utf8_lossy(&output.stdout))
    }
}

impl Default for PiDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// Interpreta a saída JSON de `pi auth check`.
pub fn parse_auth_check(stdout: &str) -> AuthState {
    let Ok(v) = serde_json::from_str::<Value>(stdout.trim()) else {
        return AuthState::Unknown;
    };
    let provider = v["provider"].as_str().unwrap_or("?");
    match v["status"].as_str() {
        Some("ready") => AuthState::Authenticated {
            account: Some(match v["authType"].as_str() {
                Some(kind) => format!("{provider}, {kind}"),
                None => provider.to_string(),
            }),
        },
        Some(_) => AuthState::Unauthenticated,
        None => AuthState::Unknown,
    }
}

#[async_trait]
impl ProviderDriver for PiDriver {
    fn id(&self) -> &'static str {
        "pi"
    }

    fn display_name(&self) -> &'static str {
        "Pi"
    }

    async fn snapshot(&self) -> ProviderSnapshot {
        let Some(binary) = self.find_binary() else {
            return ProviderSnapshot::unavailable(self.id(), self.display_name(), "`pi` não encontrado no PATH");
        };
        let version = probe_version(&binary).await;
        let (auth, message) = match self.active_model(&binary).await {
            Some((provider, model)) => (
                self.auth_for(&binary, &provider).await,
                Some(format!("modelo ativo: {provider}/{model}")),
            ),
            None => (AuthState::Unknown, Some("não foi possível consultar o modelo ativo".to_string())),
        };
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

    async fn start_session(&self, config: SessionConfig) -> Result<Box<dyn AgentSession>, SessionError> {
        let binary = self
            .find_binary()
            .ok_or_else(|| SessionError::NotReady("`pi` não encontrado no PATH".into()))?;
        Ok(Box::new(PiSession::start(&binary, &config).await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_auth_check() {
        assert_eq!(
            parse_auth_check(r#"{"status":"ready","provider":"anthropic","authType":"oauth"}"#),
            AuthState::Authenticated { account: Some("anthropic, oauth".into()) }
        );
        assert_eq!(parse_auth_check(r#"{"status":"missing","provider":"x"}"#), AuthState::Unauthenticated);
        assert_eq!(parse_auth_check("lixo"), AuthState::Unknown);
    }
}
