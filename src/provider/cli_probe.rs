use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

use super::snapshot::{AuthState, ProviderSnapshot};
use super::ProviderDriver;

const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const MODELS_TIMEOUT: Duration = Duration::from_secs(15);

/// Procura um executável no `PATH` e em diretórios extras (ex: `~/.local/bin`).
pub fn resolve_binary(names: &[&str], extra_dirs: &[PathBuf]) -> Option<PathBuf> {
    let path_dirs = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();

    path_dirs
        .iter()
        .chain(extra_dirs)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Executa `<binary> --version` com timeout e devolve a primeira linha não vazia.
pub(super) async fn probe_version(binary: &Path) -> Option<String> {
    let run = Command::new(binary).arg("--version").kill_on_drop(true).output();
    let output = tokio::time::timeout(VERSION_TIMEOUT, run).await.ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// Executa `<binary> <args>` com timeout e devolve o stdout; `None` se falhar ou estourar o tempo.
pub(super) async fn run_stdout(binary: &Path, args: &[&str]) -> Option<String> {
    let run = Command::new(binary).args(args).kill_on_drop(true).output();
    let output = tokio::time::timeout(MODELS_TIMEOUT, run).await.ok()?.ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(super) fn default_extra_dirs() -> Vec<PathBuf> {
    dirs::home_dir()
        .map(|home| vec![home.join(".local/bin"), home.join(".cargo/bin")])
        .unwrap_or_default()
}

/// Driver genérico: detecta o CLI e a versão. A autenticação só é conhecida
/// quando o driver específico do provedor existir.
pub struct CliProbeDriver {
    id: &'static str,
    display_name: &'static str,
    binaries: &'static [&'static str],
}

impl CliProbeDriver {
    pub fn new(id: &'static str, display_name: &'static str, binaries: &'static [&'static str]) -> Self {
        Self { id, display_name, binaries }
    }
}

#[async_trait]
impl ProviderDriver for CliProbeDriver {
    fn id(&self) -> &'static str {
        self.id
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    async fn snapshot(&self) -> ProviderSnapshot {
        let Some(binary) = resolve_binary(self.binaries, &default_extra_dirs()) else {
            return ProviderSnapshot::unavailable(
                self.id,
                self.display_name,
                &format!("`{}` não encontrado no PATH", self.binaries[0]),
            );
        };
        ProviderSnapshot {
            id: self.id.to_string(),
            display_name: self.display_name.to_string(),
            installed: true,
            version: probe_version(&binary).await,
            binary: Some(binary),
            auth: AuthState::Unknown,
            message: Some("autenticação ainda não verificada pelo Rusi".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn test_resolve_binary_in_extra_dir() {
        let dir = std::env::temp_dir().join(format!("rusi-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("fake-agent");
        std::fs::write(&exe, "#!/bin/sh\necho fake 1.2.3\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let plain = dir.join("not-exec");
        std::fs::write(&plain, "x").unwrap();

        assert_eq!(resolve_binary(&["fake-agent"], std::slice::from_ref(&dir)), Some(exe));
        assert_eq!(resolve_binary(&["not-exec"], std::slice::from_ref(&dir)), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_probe_version_reads_first_line() {
        let dir = std::env::temp_dir().join(format!("rusi-ver-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("fake-agent");
        std::fs::write(&exe, "#!/bin/sh\necho fake 1.2.3\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(probe_version(&exe).await.as_deref(), Some("fake 1.2.3"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_missing_binary_is_unavailable() {
        let driver = CliProbeDriver::new("ghost", "Ghost", &["rusi-definitely-not-installed"]);
        let snap = driver.snapshot().await;
        assert!(!snap.installed);
        assert_eq!(snap.auth, AuthState::Unavailable);
    }
}
