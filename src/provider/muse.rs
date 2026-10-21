use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

use super::cli_probe::{default_extra_dirs, probe_version, resolve_binary};
use super::muse_session::MuseSession;
use super::snapshot::{AuthState, ProviderSnapshot};
use super::ProviderDriver;
use crate::rpc::RpcPeer;
use crate::session::{AgentSession, SessionConfig, SessionError};

const CATALOG_TIMEOUT: Duration = Duration::from_secs(12);

/// Driver do Muse Code: usa o CLI `muse` (`muse serve`, protocolo MSP) e o login Meta dele.
pub struct MuseDriver {
    binary: Option<PathBuf>,
}

impl MuseDriver {
    pub fn new() -> Self {
        Self { binary: None }
    }

    pub fn with_binary(mut self, binary: PathBuf) -> Self {
        self.binary = Some(binary);
        self
    }

    fn find_binary(&self) -> Option<PathBuf> {
        self.binary.clone().or_else(|| resolve_binary(&["muse"], &default_extra_dirs()))
    }

    /// Ids dos modelos que o host lista (`model/list`); `None` se o host não respondeu.
    async fn model_ids(&self, binary: &PathBuf) -> Option<Vec<String>> {
        let probe = async {
            let mut cmd = Command::new(binary);
            cmd.args(["serve", "--no-session-log"]).env("MUSE_NO_AUTO_UPDATE", "1");
            let (peer, _incoming) = RpcPeer::spawn(cmd, true).ok()?;
            let result = async {
                peer.request(
                    "initialize",
                    json!({ "clientInfo": { "name": "rusi", "version": env!("CARGO_PKG_VERSION") }, "capabilities": {} }),
                )
                .await
                .ok()?;
                peer.notify("initialized", None).await.ok()?;
                let models = peer.request("model/list", json!({})).await.ok()?;
                let models = models["models"].as_array()?;
                Some(models.iter().filter_map(|m| m["modelId"].as_str().map(String::from)).collect())
            }
            .await;
            peer.close().await;
            result
        };
        tokio::time::timeout(CATALOG_TIMEOUT, probe).await.ok().flatten()
    }
}

impl Default for MuseDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ProviderDriver for MuseDriver {
    fn id(&self) -> &'static str {
        "muse"
    }

    fn display_name(&self) -> &'static str {
        "Muse Code"
    }

    async fn snapshot(&self) -> ProviderSnapshot {
        let Some(binary) = self.find_binary() else {
            return ProviderSnapshot::unavailable(
                self.id(),
                self.display_name(),
                "`muse` não encontrado; instale o Muse Code e execute `muse login`",
            );
        };
        let (version, models) = tokio::join!(probe_version(&binary), self.model_ids(&binary));
        // Como no T3, o `muse serve` não expõe o estado do login: só dá para saber se o
        // host responde e lista modelos. O erro de login aparece no primeiro turno.
        let message = match models {
            Some(ids) if ids.is_empty() => Some("nenhum modelo listado; execute `muse login`".to_string()),
            None => Some("o host do Muse não respondeu; verifique a instalação".to_string()),
            Some(_) => None,
        };
        ProviderSnapshot {
            id: self.id().to_string(),
            display_name: self.display_name().to_string(),
            installed: true,
            version,
            binary: Some(binary),
            auth: AuthState::Unknown,
            message,
        }
    }

    async fn models(&self) -> Vec<String> {
        let Some(binary) = self.find_binary() else { return Vec::new() };
        self.model_ids(&binary).await.unwrap_or_default()
    }

    async fn start_session(&self, config: SessionConfig) -> Result<Box<dyn AgentSession>, SessionError> {
        let binary = self
            .find_binary()
            .ok_or_else(|| SessionError::NotReady("`muse` não encontrado no PATH".into()))?;
        Ok(Box::new(MuseSession::start(&binary, &config).await?))
    }
}
