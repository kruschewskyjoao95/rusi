//! Cofre de segredos. O padrão é o keyring do SO (Secret Service no Linux);
//! tokens nunca são gravados em disco pelo Rusi.

use std::collections::HashMap;
use std::sync::Mutex;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("Cofre de segredos indisponível: {0}")]
    Unavailable(String),

    #[error("Falha no cofre de segredos: {0}")]
    Backend(String),
}

/// Armazena segredos por `(provedor, chave)`.
pub trait SecretStore: Send + Sync {
    fn set(&self, provider: &str, key: &str, value: &str) -> Result<(), SecretError>;
    fn get(&self, provider: &str, key: &str) -> Result<Option<String>, SecretError>;
    /// Devolve se o segredo existia.
    fn delete(&self, provider: &str, key: &str) -> Result<bool, SecretError>;
}

/// Segredos no keyring do sistema operacional.
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    pub fn new() -> Self {
        Self::with_service("rusi")
    }

    pub fn with_service(service: &str) -> Self {
        Self { service: service.to_string() }
    }

    fn entry(&self, provider: &str, key: &str) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(&self.service, &entry_name(provider, key)).map_err(map_err)
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

fn entry_name(provider: &str, key: &str) -> String {
    format!("{provider}/{key}")
}

fn map_err(err: keyring::Error) -> SecretError {
    match err {
        keyring::Error::NoDefaultStore | keyring::Error::NoStorageAccess(_) => {
            SecretError::Unavailable(err.to_string())
        }
        other => SecretError::Backend(other.to_string()),
    }
}

impl SecretStore for KeyringStore {
    fn set(&self, provider: &str, key: &str, value: &str) -> Result<(), SecretError> {
        self.entry(provider, key)?.set_password(value).map_err(map_err)
    }

    fn get(&self, provider: &str, key: &str) -> Result<Option<String>, SecretError> {
        match self.entry(provider, key)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(map_err(err)),
        }
    }

    fn delete(&self, provider: &str, key: &str) -> Result<bool, SecretError> {
        match self.entry(provider, key)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(err) => Err(map_err(err)),
        }
    }
}

/// Cofre em memória, para testes e para quando não há keyring.
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemoryStore {
    fn set(&self, provider: &str, key: &str, value: &str) -> Result<(), SecretError> {
        self.lock().insert(entry_name(provider, key), value.to_string());
        Ok(())
    }

    fn get(&self, provider: &str, key: &str) -> Result<Option<String>, SecretError> {
        Ok(self.lock().get(&entry_name(provider, key)).cloned())
    }

    fn delete(&self, provider: &str, key: &str) -> Result<bool, SecretError> {
        Ok(self.lock().remove(&entry_name(provider, key)).is_some())
    }
}

impl MemoryStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.entries.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_store_roundtrip() {
        let store = MemoryStore::default();
        assert_eq!(store.get("codex", "token").unwrap(), None);
        store.set("codex", "token", "s3cret").unwrap();
        assert_eq!(store.get("codex", "token").unwrap().as_deref(), Some("s3cret"));
        assert_eq!(store.get("claude", "token").unwrap(), None, "isolado por provedor");
        assert!(store.delete("codex", "token").unwrap());
        assert!(!store.delete("codex", "token").unwrap());
    }

    /// Exercita o keyring real; só roda com `RUSI_TEST_KEYRING=1` por exigir Secret Service ativo.
    #[test]
    fn test_keyring_store_roundtrip_when_enabled() {
        if std::env::var("RUSI_TEST_KEYRING").as_deref() != Ok("1") {
            return;
        }
        let store = KeyringStore::with_service("rusi-test");
        store.set("p", "k", "v").unwrap();
        assert_eq!(store.get("p", "k").unwrap().as_deref(), Some("v"));
        assert!(store.delete("p", "k").unwrap());
        assert_eq!(store.get("p", "k").unwrap(), None);
    }
}
