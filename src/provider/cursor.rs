use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

use super::cli_probe::{default_extra_dirs, probe_version, resolve_binary};
use super::cursor_session;
use super::snapshot::{AuthState, ProviderSnapshot};
use super::ProviderDriver;
use crate::session::{AgentSession, SessionConfig, SessionError};

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const BINARIES: &[&str] = &["cursor-agent", "agent"];

/// Driver do Cursor: `cursor-agent -p --output-format stream-json`, um processo por turno.
pub struct CursorDriver {
    binary: Option<PathBuf>,
}

impl CursorDriver {
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

    async fn read_auth(&self, binary: &PathBuf) -> AuthState {
        if std::env::var("CURSOR_API_KEY").is_ok_and(|k| !k.trim().is_empty()) {
            return AuthState::Authenticated { account: Some("CURSOR_API_KEY".to_string()) };
        }
        let run = Command::new(binary).arg("status").kill_on_drop(true).output();
        match tokio::time::timeout(PROBE_TIMEOUT, run).await {
            Ok(Ok(output)) => {
                let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
                parse_status_output(&text)
            }
            _ => AuthState::Unknown,
        }
    }
}

impl Default for CursorDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// Interpreta a saída de `cursor-agent status`.
pub fn parse_status_output(output: &str) -> AuthState {
    let plain = strip_ansi(output);
    let lower = plain.to_lowercase();
    if lower.contains("not logged in") || lower.contains("not authenticated") {
        return AuthState::Unauthenticated;
    }
    if lower.contains("logged in") {
        let account = plain
            .lines()
            .find_map(|l| l.split_once("as ").map(|(_, rest)| rest.trim().to_string()))
            .filter(|a| !a.is_empty());
        return AuthState::Authenticated { account };
    }
    AuthState::Unknown
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[async_trait]
impl ProviderDriver for CursorDriver {
    fn id(&self) -> &'static str {
        "cursor"
    }

    fn display_name(&self) -> &'static str {
        "Cursor"
    }

    async fn snapshot(&self) -> ProviderSnapshot {
        let Some(binary) = self.find_binary() else {
            return ProviderSnapshot::unavailable(self.id(), self.display_name(), "`cursor-agent` não encontrado no PATH");
        };
        let (version, auth) = tokio::join!(probe_version(&binary), self.read_auth(&binary));
        let message = (auth == AuthState::Unauthenticated).then(|| "execute `cursor-agent login`".to_string());
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
            .ok_or_else(|| SessionError::NotReady("`cursor-agent` não encontrado no PATH".into()))?;
        if self.binary.is_none() && self.read_auth(&binary).await == AuthState::Unauthenticated {
            return Err(SessionError::NotReady("Cursor sem login: execute `cursor-agent login`".into()));
        }
        Ok(Box::new(cursor_session::session(&binary, &config)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_status_output() {
        assert_eq!(parse_status_output("Not logged in\n"), AuthState::Unauthenticated);
        assert_eq!(parse_status_output("\u{1b}[1mNot logged in\u{1b}[0m"), AuthState::Unauthenticated);
        assert_eq!(
            parse_status_output("✓ Logged in as joao@example.com\n"),
            AuthState::Authenticated { account: Some("joao@example.com".into()) }
        );
        assert_eq!(parse_status_output("???"), AuthState::Unknown);
    }
}
