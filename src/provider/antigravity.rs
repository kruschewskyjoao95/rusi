//! Driver do Antigravity (Google): o `agy_acp_server` oficial, via ACP, com um perfil
//! privado (`GEMINI_HOME`) onde ele guarda `antigravity-acp/acp_token.json`.
//!
//! Este módulo cobre detecção do binário, estado do login e sessões. O download do binário
//! e o login OAuth (redirect loopback) pertencem à fase 4 do roadmap.

use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};

use super::acp_session::{AcpOptions, AcpSession};
use super::cli_probe::resolve_binary;
use super::snapshot::{AuthState, ProviderSnapshot};
use super::ProviderDriver;
use crate::config::AgentMode;
use crate::session::{AgentSession, SessionConfig, SessionError};

const PERSONAL_AUTH_METHOD: &str = "oauth-personal";
const SIGN_IN_HINT: &str =
    "Antigravity sem login: o login Google do Rusi ainda não foi implementado (fase 4); \
     o Antigravity exige uma assinatura elegível na conta Google";

/// Nomes do executável do servidor ACP, na ordem de preferência.
const EXECUTABLE_NAMES: &[&str] = &["agy_acp_server", "agy-acp-server"];
/// O servidor precisa do harness ao lado dele.
const HARNESS_NAME: &str = "localharness_external";

pub struct AntigravityDriver {
    binary: Option<PathBuf>,
    profile_dir: Option<PathBuf>,
}

impl AntigravityDriver {
    pub fn new() -> Self {
        Self { binary: None, profile_dir: None }
    }

    pub fn with_binary(mut self, binary: PathBuf) -> Self {
        self.binary = Some(binary);
        self
    }

    pub fn with_profile_dir(mut self, dir: PathBuf) -> Self {
        self.profile_dir = Some(dir);
        self
    }

    /// Perfil privado do Rusi para o Antigravity (nunca o `~/.gemini` do usuário).
    fn profile_dir(&self) -> Option<PathBuf> {
        self.profile_dir
            .clone()
            .or_else(|| dirs::data_dir().map(|d| d.join("rusi").join("profiles").join("antigravity")))
    }

    /// Instalação gerenciada pelo Rusi primeiro, depois PATH.
    fn find_binary(&self) -> Option<PathBuf> {
        if let Some(binary) = &self.binary {
            return Some(binary.clone());
        }
        let managed: Vec<PathBuf> = dirs::data_dir()
            .map(|d| vec![d.join("rusi").join("installs").join("antigravity")])
            .unwrap_or_default();
        resolve_binary(EXECUTABLE_NAMES, &managed)
    }
}

impl Default for AntigravityDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// Caminho do harness ao lado do executável; `None` se não existir.
pub fn harness_for(executable: &Path) -> Option<PathBuf> {
    let harness = executable.parent()?.join(HARNESS_NAME);
    harness.is_file().then_some(harness)
}

/// Lê o `acp_token.json` só para saber se há login; o conteúdo nunca sai daqui.
pub fn parse_token_file(raw: &str) -> AuthState {
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return AuthState::Unknown;
    };
    if v["refresh_token"].as_str().is_some_and(|t| !t.is_empty()) {
        AuthState::Authenticated { account: Some("Google account".to_string()) }
    } else {
        AuthState::Unauthenticated
    }
}

fn token_path(profile_dir: &Path) -> PathBuf {
    profile_dir.join("antigravity-acp").join("acp_token.json")
}

/// Variáveis de ambiente do agente: perfil isolado, armazenamento em arquivo e sem navegador.
pub(super) fn agent_env(profile_dir: &Path, harness: &Path, base_home: Option<&str>) -> Vec<(String, String)> {
    let tmp = profile_dir.join("antigravity-acp").join("tmp");
    let mut env = vec![
        ("GEMINI_HOME".to_string(), profile_dir.display().to_string()),
        ("AGY_ACP_FORCE_FILE_STORAGE".to_string(), "1".to_string()),
        // Sessões normais não podem abrir o navegador; o login (fase 4) é um fluxo à parte.
        ("BROWSER".to_string(), "/bin/false".to_string()),
        ("PYTHONUNBUFFERED".to_string(), "1".to_string()),
        ("ELECTRON_RUN_AS_NODE".to_string(), "1".to_string()),
        ("TMPDIR".to_string(), tmp.display().to_string()),
        ("ANTIGRAVITY_HARNESS_PATH".to_string(), harness.display().to_string()),
    ];
    if let Some(home) = base_home {
        env.push(("HOME".to_string(), home.to_string()));
    }
    env
}

fn prepare_profile(profile_dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for dir in [profile_dir.to_path_buf(), profile_dir.join("antigravity-acp"), profile_dir.join("antigravity-acp").join("tmp")] {
        std::fs::create_dir_all(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Modo de sessão do Antigravity para o modo do Rusi.
pub(super) fn session_mode_for(config: &SessionConfig) -> &'static str {
    if config.dangerously_skip_permissions { "yolo" } else { "default" }
}

fn no_auth_method(_initialized: &Value) -> Option<String> {
    None
}

#[async_trait]
impl ProviderDriver for AntigravityDriver {
    fn id(&self) -> &'static str {
        "antigravity"
    }

    fn display_name(&self) -> &'static str {
        "Antigravity (Google)"
    }

    async fn snapshot(&self) -> ProviderSnapshot {
        let Some(binary) = self.find_binary() else {
            return ProviderSnapshot::unavailable(
                self.id(),
                self.display_name(),
                "`agy_acp_server` não instalado (a instalação gerenciada entra na fase 4)",
            );
        };
        let auth = match self.profile_dir().map(|d| token_path(&d)).map(std::fs::read_to_string) {
            Some(Ok(raw)) => parse_token_file(&raw),
            Some(Err(err)) if err.kind() == std::io::ErrorKind::NotFound => AuthState::Unauthenticated,
            _ => AuthState::Unknown,
        };
        let message = match (&auth, harness_for(&binary)) {
            (_, None) => Some(format!("falta `{HARNESS_NAME}` ao lado do executável")),
            (AuthState::Unauthenticated, _) => Some(SIGN_IN_HINT.to_string()),
            _ => None,
        };
        ProviderSnapshot {
            id: self.id().to_string(),
            display_name: self.display_name().to_string(),
            installed: true,
            version: None,
            binary: Some(binary),
            auth,
            message,
        }
    }

    async fn start_session(&self, config: SessionConfig) -> Result<Box<dyn AgentSession>, SessionError> {
        let binary = self.find_binary().ok_or_else(|| {
            SessionError::NotReady("`agy_acp_server` não instalado (a instalação gerenciada entra na fase 4)".into())
        })?;
        let harness = harness_for(&binary).ok_or_else(|| {
            SessionError::NotReady(format!("falta `{HARNESS_NAME}` ao lado de {}", binary.display()))
        })?;
        let profile_dir = self
            .profile_dir()
            .ok_or_else(|| SessionError::NotReady("sem diretório de dados do usuário para o perfil".into()))?;
        prepare_profile(&profile_dir)?;

        let options = AcpOptions {
            binary,
            args: if cfg!(target_os = "linux") { vec!["--uid=".to_string()] } else { Vec::new() },
            env: agent_env(&profile_dir, &harness, None),
            init_meta: None,
            auto_accept: config.dangerously_skip_permissions || config.mode == AgentMode::Auto,
            plan_mode: config.mode == AgentMode::Plan && !config.dangerously_skip_permissions,
            auth_method: no_auth_method,
            login_hint: SIGN_IN_HINT.to_string(),
            eager_auth_method: Some(PERSONAL_AUTH_METHOD.to_string()),
            prefer_resume: true,
            session_mode: Some(session_mode_for(&config)),
        };
        Ok(Box::new(AcpSession::start(options, &config).await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_token_file() {
        assert!(matches!(
            parse_token_file(r#"{"client_id":"x","client_secret":"y","refresh_token":"r"}"#),
            AuthState::Authenticated { .. }
        ));
        assert_eq!(parse_token_file(r#"{"refresh_token":""}"#), AuthState::Unauthenticated);
        assert_eq!(parse_token_file("lixo"), AuthState::Unknown);
    }

    #[test]
    fn test_session_mode_mapping() {
        assert_eq!(session_mode_for(&SessionConfig::default()), "default");
        assert_eq!(
            session_mode_for(&SessionConfig { dangerously_skip_permissions: true, ..Default::default() }),
            "yolo"
        );
    }

    #[test]
    fn test_env_isolates_profile_and_blocks_browser() {
        let env = agent_env(Path::new("/p"), Path::new("/i/localharness_external"), None);
        let get = |k: &str| env.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("GEMINI_HOME"), Some("/p"));
        assert_eq!(get("BROWSER"), Some("/bin/false"));
        assert_eq!(get("ANTIGRAVITY_HARNESS_PATH"), Some("/i/localharness_external"));
        assert_eq!(get("TMPDIR"), Some("/p/antigravity-acp/tmp"));
        assert_eq!(get("AGY_ACP_FORCE_FILE_STORAGE"), Some("1"));
    }

    #[test]
    fn test_harness_must_sit_next_to_executable() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("agy_acp_server");
        std::fs::write(&exe, "").unwrap();
        assert_eq!(harness_for(&exe), None);
        std::fs::write(dir.path().join(HARNESS_NAME), "").unwrap();
        assert_eq!(harness_for(&exe), Some(dir.path().join(HARNESS_NAME)));
    }
}
