use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Estado de autenticação de um provedor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum AuthState {
    /// Login válido; `account` é um identificador não sensível (e-mail, plano).
    Authenticated { account: Option<String> },
    /// O CLI existe, mas não há login.
    Unauthenticated,
    /// Não dá para saber sem iniciar uma sessão com o CLI.
    Unknown,
    /// CLI não instalado.
    Unavailable,
}

/// Retrato do estado de um provedor num dado instante.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSnapshot {
    pub id: String,
    pub display_name: String,
    pub installed: bool,
    pub version: Option<String>,
    pub binary: Option<PathBuf>,
    pub auth: AuthState,
    pub message: Option<String>,
}

impl ProviderSnapshot {
    pub fn unavailable(id: &str, display_name: &str, hint: &str) -> Self {
        Self {
            id: id.to_string(),
            display_name: display_name.to_string(),
            installed: false,
            version: None,
            binary: None,
            auth: AuthState::Unavailable,
            message: Some(hint.to_string()),
        }
    }
}
