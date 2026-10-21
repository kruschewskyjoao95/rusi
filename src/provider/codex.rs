use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

use super::cli_probe::{default_extra_dirs, probe_version, resolve_binary, run_stdout};
use super::codex_session::CodexSession;
use super::snapshot::{AuthState, ProviderSnapshot};
use super::ProviderDriver;
use crate::rpc::RpcPeer;
use crate::session::{AgentSession, SessionConfig, SessionError};

const AUTH_PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// Driver do Codex: usa o CLI oficial `codex` (`codex app-server`) e o login ChatGPT dele.
pub struct CodexDriver {
    binary: Option<PathBuf>,
}

impl CodexDriver {
    pub fn new() -> Self {
        Self { binary: None }
    }

    pub fn with_binary(mut self, binary: PathBuf) -> Self {
        self.binary = Some(binary);
        self
    }

    fn find_binary(&self) -> Option<PathBuf> {
        self.binary.clone().or_else(|| resolve_binary(&["codex"], &default_extra_dirs()))
    }

    /// Pergunta ao app-server quem está logado (`account/read`).
    async fn read_auth(&self, binary: &PathBuf) -> AuthState {
        let probe = async {
            let mut cmd = Command::new(binary);
            cmd.arg("app-server");
            let (peer, _incoming) = RpcPeer::spawn(cmd, false).ok()?;
            let result = async {
                peer.request(
                    "initialize",
                    json!({ "clientInfo": { "name": "rusi", "title": "Rusi", "version": env!("CARGO_PKG_VERSION") } }),
                )
                .await
                .ok()?;
                peer.notify("initialized", None).await.ok()?;
                peer.request("account/read", json!({ "refreshToken": false })).await.ok()
            }
            .await;
            peer.close().await;
            result
        };
        match tokio::time::timeout(AUTH_PROBE_TIMEOUT, probe).await {
            Ok(Some(response)) => parse_account(&response),
            _ => AuthState::Unknown,
        }
    }
}

impl Default for CodexDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// Interpreta a resposta de `account/read`.
pub fn parse_account(response: &Value) -> AuthState {
    let account = &response["account"];
    match account["type"].as_str() {
        Some("chatgpt") => {
            let email = account["email"].as_str();
            let plan = account["planType"].as_str();
            let label = match (email, plan) {
                (Some(e), Some(p)) => Some(format!("{e}, {p}")),
                (Some(x), None) | (None, Some(x)) => Some(x.to_string()),
                (None, None) => None,
            };
            AuthState::Authenticated { account: label }
        }
        Some("apiKey") => AuthState::Authenticated { account: Some("API key".to_string()) },
        Some(other) => AuthState::Authenticated { account: Some(other.to_string()) },
        None if response["requiresOpenaiAuth"].as_bool() == Some(true) => AuthState::Unauthenticated,
        None => AuthState::Unknown,
    }
}

/// Interpreta `codex debug models`: só os modelos visíveis no seletor do próprio Codex.
pub fn parse_models(raw: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    value["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["visibility"].as_str() == Some("list"))
        .filter_map(|m| m["slug"].as_str().map(String::from))
        .collect()
}

#[async_trait]
impl ProviderDriver for CodexDriver {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn display_name(&self) -> &'static str {
        "Codex (ChatGPT)"
    }

    async fn snapshot(&self) -> ProviderSnapshot {
        let Some(binary) = self.find_binary() else {
            return ProviderSnapshot::unavailable(self.id(), self.display_name(), "`codex` não encontrado no PATH");
        };
        let (version, auth) = tokio::join!(probe_version(&binary), self.read_auth(&binary));
        let message = match auth {
            AuthState::Unauthenticated => Some("execute `codex login`".to_string()),
            AuthState::Unknown => Some("não foi possível consultar a conta".to_string()),
            _ => None,
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

    async fn models(&self) -> Vec<String> {
        let Some(binary) = self.find_binary() else { return Vec::new() };
        run_stdout(&binary, &["debug", "models"]).await.map(|raw| parse_models(&raw)).unwrap_or_default()
    }

    async fn start_session(&self, config: SessionConfig) -> Result<Box<dyn AgentSession>, SessionError> {
        let binary = self
            .find_binary()
            .ok_or_else(|| SessionError::NotReady("`codex` não encontrado no PATH".into()))?;
        if self.binary.is_none() && self.read_auth(&binary).await == AuthState::Unauthenticated {
            return Err(SessionError::NotReady("Codex sem login: execute `codex login`".into()));
        }
        Ok(Box::new(CodexSession::start(&binary, &config).await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_models_keeps_only_listed() {
        let raw = r#"{"models":[{"slug":"a","visibility":"list"},{"slug":"b","visibility":"hide"},{"slug":"c","visibility":"list"}]}"#;
        assert_eq!(parse_models(raw), ["a", "c"]);
        assert!(parse_models("não é json").is_empty());
    }

    #[test]
    fn test_parse_account_variants() {
        assert_eq!(
            parse_account(&json!({"account": {"type": "chatgpt", "email": "a@b.c", "planType": "plus"}, "requiresOpenaiAuth": true})),
            AuthState::Authenticated { account: Some("a@b.c, plus".into()) }
        );
        assert_eq!(
            parse_account(&json!({"account": {"type": "apiKey"}, "requiresOpenaiAuth": true})),
            AuthState::Authenticated { account: Some("API key".into()) }
        );
        assert_eq!(parse_account(&json!({"account": null, "requiresOpenaiAuth": true})), AuthState::Unauthenticated);
        assert_eq!(parse_account(&json!({"account": null, "requiresOpenaiAuth": false})), AuthState::Unknown);
    }
}
