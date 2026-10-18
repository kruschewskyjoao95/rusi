//! Persistência local em SQLite: threads (sessões) e seus eventos.

mod migrations;
mod threads;

pub use threads::{NewThread, Thread, ThreadEvent};

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use thiserror::Error;

use migrations::MIGRATIONS;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("Erro de SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Erro de I/O ao preparar o banco: {0}")]
    Io(#[from] std::io::Error),

    #[error("Payload de evento inválido: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Thread não encontrada: {0}")]
    ThreadNotFound(String),

    #[error("O banco está na versão {found}, mais nova que a suportada ({supported}); atualize o Rusi")]
    SchemaTooNew { found: usize, supported: usize },
}

/// Banco de dados do Rusi. Clonar é barato: todas as cópias dividem a mesma conexão.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// Caminho padrão: `$RUSI_DB_PATH` ou `~/.local/share/rusi/rusi.db`.
    pub fn default_path() -> Option<PathBuf> {
        std::env::var_os("RUSI_DB_PATH")
            .map(PathBuf::from)
            .or_else(|| dirs::data_dir().map(|d| d.join("rusi").join("rusi.db")))
    }

    /// Abre (criando diretório e arquivo, com permissão 0600) e migra o banco.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let existed = path.exists();
        let conn = Connection::open(path)?;
        if !existed {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Self::from_connection(conn)
    }

    /// Banco em memória, para testes.
    pub fn open_in_memory() -> Result<Self, DbError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(mut conn: Connection) -> Result<Self, DbError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&mut conn)?;
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    /// Versão atual do schema.
    pub fn schema_version(&self) -> Result<usize, DbError> {
        Ok(self.conn().query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as usize)
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn migrate(conn: &mut Connection) -> Result<(), DbError> {
    let current = conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as usize;
    if current > MIGRATIONS.len() {
        return Err(DbError::SchemaTooNew { found: current, supported: MIGRATIONS.len() });
    }
    for (index, (_name, sql)) in MIGRATIONS.iter().enumerate().skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (index + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fresh_database_is_fully_migrated() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len());
    }

    #[test]
    fn test_migration_is_incremental_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rusi.db");

        // Simula um banco antigo parado na migration 1.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0].1).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            conn.execute(
                "INSERT INTO threads (id, provider, created_at, updated_at) VALUES ('t1','claude',1,1)",
                [],
            )
            .unwrap();
        }

        let db = Database::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len());
        assert!(!db.get_thread("t1").unwrap().pinned, "dados antigos preservados, pinned=false");

        // Reabrir não reaplica nada.
        drop(db);
        assert_eq!(Database::open(&path).unwrap().schema_version().unwrap(), MIGRATIONS.len());
    }

    #[test]
    fn test_newer_schema_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rusi.db");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", (MIGRATIONS.len() + 1) as i64)
            .unwrap();
        assert!(matches!(Database::open(&path), Err(DbError::SchemaTooNew { .. })));
    }

    #[test]
    fn test_new_database_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("rusi.db");
        Database::open(&path).unwrap();
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }
}
