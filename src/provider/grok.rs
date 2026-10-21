use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

use super::acp_session::{AcpOptions, AcpSession};
use super::cli_probe::{default_extra_dirs, probe_version, resolve_binary, run_stdout};
use super::snapshot::{AuthState, ProviderSnapshot};
use super::ProviderDriver;
use crate::config::AgentMode;
use crate::session::{AgentSession, SessionConfig, SessionError};

const API_KEY_ENV: &str = "XAI_API_KEY";
const API_KEY_AUTH_METHOD: &str = "xai.api_key";
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// Driver do Grok: `grok agent stdio` (ACP) com o login do próprio `grok`.
pub struct GrokDriver {
    binary: Option<PathBuf>,
}

impl GrokDriver {
    pub fn new() -> Self {
        Self { binary: None }
    }

    pub fn with_binary(mut self, binary: PathBuf) -> Self {
        self.binary = Some(binary);
        self
    }

    fn find_binary(&self) -> Option<PathBuf> {
        self.binary.clone().or_else(|| resolve_binary(&["grok"], &default_extra_dirs()))
    }

    async fn read_auth(&self, binary: &PathBuf) -> AuthState {
        if std::env::var(API_KEY_ENV).is_ok_and(|k| !k.trim().is_empty()) {
            return AuthState::Authenticated { account: Some("XAI_API_KEY".to_string()) };
        }
        // `grok models` informa o login sem abrir navegador nem subir sessão.
        let run = Command::new(binary).arg("models").kill_on_drop(true).output();
        match tokio::time::timeout(PROBE_TIMEOUT, run).await {
            Ok(Ok(output)) => parse_models_output(&String::from_utf8_lossy(&output.stdout)),
            _ => AuthState::Unknown,
        }
    }
}

impl Default for GrokDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// Interpreta a saída de `grok models` (mesma heurística do T3).
pub fn parse_models_output(output: &str) -> AuthState {
    let lower = output.to_lowercase();
    if lower.contains("you are logged in") {
        AuthState::Authenticated { account: Some("grok.com".to_string()) }
    } else if lower.contains("not authenticated") || lower.contains("not logged in") {
        AuthState::Unauthenticated
    } else {
        AuthState::Unknown
    }
}

/// Os modelos de `grok models`: as linhas `- id` / `* id (default)` depois de "Available models:".
pub fn parse_model_list(output: &str) -> Vec<String> {
    output
        .lines()
        .skip_while(|l| !l.contains("Available models"))
        .skip(1)
        .filter_map(|l| l.trim().strip_prefix(['-', '*']))
        .filter_map(|l| l.split_whitespace().next().map(String::from))
        .collect()
}

/// Argumentos de lançamento por modo. `--permission-mode` na linha de comando vence a
/// configuração do usuário, então `accept-edits` não herda um "always approve" configurado.
pub(super) fn spawn_args(config: &SessionConfig) -> Vec<String> {
    let args: &[&str] = if config.dangerously_skip_permissions {
        &["agent", "--always-approve", "stdio"]
    } else if config.mode == AgentMode::Auto {
        &["--permission-mode", "auto", "agent", "stdio"]
    } else {
        &["--permission-mode", "default", "agent", "stdio"]
    };
    args.iter().map(|s| s.to_string()).collect()
}

fn auth_method(_initialized: &Value) -> Option<String> {
    std::env::var(API_KEY_ENV)
        .is_ok_and(|k| !k.trim().is_empty())
        .then(|| API_KEY_AUTH_METHOD.to_string())
}

#[async_trait]
impl ProviderDriver for GrokDriver {
    fn id(&self) -> &'static str {
        "grok"
    }

    fn display_name(&self) -> &'static str {
        "Grok"
    }

    async fn snapshot(&self) -> ProviderSnapshot {
        let Some(binary) = self.find_binary() else {
            return ProviderSnapshot::unavailable(self.id(), self.display_name(), "`grok` não encontrado no PATH");
        };
        let (version, auth) = tokio::join!(probe_version(&binary), self.read_auth(&binary));
        let message = (auth == AuthState::Unauthenticated).then(|| "execute `grok login`".to_string());
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
        run_stdout(&binary, &["models"]).await.map(|out| parse_model_list(&out)).unwrap_or_default()
    }

    async fn start_session(&self, config: SessionConfig) -> Result<Box<dyn AgentSession>, SessionError> {
        let binary = self
            .find_binary()
            .ok_or_else(|| SessionError::NotReady("`grok` não encontrado no PATH".into()))?;
        let options = AcpOptions {
            binary,
            args: spawn_args(&config),
            env: Vec::new(),
            // Sem isso, no modo `auto` o Grok nega em silêncio o que o classificador bloqueia.
            init_meta: Some(serde_json::json!({ "clientType": "extension" })),
            auto_accept: config.dangerously_skip_permissions || config.mode == AgentMode::Auto,
            plan_mode: config.mode == AgentMode::Plan && !config.dangerously_skip_permissions,
            auth_method,
            login_hint: "Grok sem login: execute `grok login`".to_string(),
            eager_auth_method: None,
            prefer_resume: false,
            session_mode: None,
        };
        Ok(Box::new(AcpSession::start(options, &config).await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_models_output() {
        assert!(matches!(parse_models_output("You are logged in as x\nmodels"), AuthState::Authenticated { .. }));
        assert_eq!(parse_models_output("You are not authenticated.\n"), AuthState::Unauthenticated);
        assert_eq!(parse_models_output("???"), AuthState::Unknown);
    }

    #[test]
    fn test_parse_model_list() {
        let out = "You are logged in with grok.com.\n\nDefault model: grok-4.6\n\nAvailable models:\n  - grok-4.7\n  * grok-4.6 (default)\n";
        assert_eq!(parse_model_list(out), ["grok-4.7", "grok-4.6"]);
        assert!(parse_model_list("Not logged in").is_empty());
    }

    #[test]
    fn test_spawn_args_by_mode() {
        let args = |mode, skip| {
            spawn_args(&SessionConfig { mode, dangerously_skip_permissions: skip, ..Default::default() }).join(" ")
        };
        assert_eq!(args(AgentMode::AcceptEdits, false), "--permission-mode default agent stdio");
        assert_eq!(args(AgentMode::Plan, false), "--permission-mode default agent stdio");
        assert_eq!(args(AgentMode::Auto, false), "--permission-mode auto agent stdio");
        assert_eq!(args(AgentMode::Plan, true), "agent --always-approve stdio");
    }
}
