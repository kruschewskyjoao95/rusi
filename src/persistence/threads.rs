use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use super::{Database, DbError, now_millis};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub provider: String,
    pub model: Option<String>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub pinned: bool,
    /// Id da conversa dentro do provedor (ex: sessão do `claude`), usado para retomar.
    pub provider_session_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default)]
pub struct NewThread {
    pub provider: String,
    pub model: Option<String>,
    pub title: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadEvent {
    pub seq: i64,
    pub kind: String,
    pub payload: serde_json::Value,
    pub created_at: i64,
}

const THREAD_COLUMNS: &str = "id, provider, model, title, cwd, pinned, created_at, updated_at, provider_session_id";

fn thread_from_row(row: &Row<'_>) -> rusqlite::Result<Thread> {
    Ok(Thread {
        id: row.get(0)?,
        provider: row.get(1)?,
        model: row.get(2)?,
        title: row.get(3)?,
        cwd: row.get(4)?,
        pinned: row.get::<_, i64>(5)? != 0,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        provider_session_id: row.get(8)?,
    })
}

impl Database {
    pub fn create_thread(&self, new: NewThread) -> Result<Thread, DbError> {
        let now = now_millis();
        let id = uuid::Uuid::new_v4().to_string();
        self.conn().execute(
            "INSERT INTO threads (id, provider, model, title, cwd, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![id, new.provider, new.model, new.title, new.cwd, now],
        )?;
        self.get_thread(&id)
    }

    pub fn get_thread(&self, id: &str) -> Result<Thread, DbError> {
        self.conn()
            .query_row(
                &format!("SELECT {THREAD_COLUMNS} FROM threads WHERE id = ?1"),
                [id],
                thread_from_row,
            )
            .optional()?
            .ok_or_else(|| DbError::ThreadNotFound(id.to_string()))
    }

    /// Threads fixadas primeiro, depois as mais recentemente atualizadas.
    pub fn list_threads(&self, limit: usize) -> Result<Vec<Thread>, DbError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {THREAD_COLUMNS} FROM threads ORDER BY pinned DESC, updated_at DESC, rowid DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map([limit as i64], thread_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Thread mais recente (para `--continue`).
    pub fn latest_thread(&self) -> Result<Option<Thread>, DbError> {
        Ok(self
            .conn()
            .query_row(
                &format!("SELECT {THREAD_COLUMNS} FROM threads ORDER BY updated_at DESC, rowid DESC LIMIT 1"),
                [],
                thread_from_row,
            )
            .optional()?)
    }

    pub fn set_thread_pinned(&self, id: &str, pinned: bool) -> Result<(), DbError> {
        let changed = self
            .conn()
            .execute("UPDATE threads SET pinned = ?2 WHERE id = ?1", params![id, pinned as i64])?;
        if changed == 0 {
            return Err(DbError::ThreadNotFound(id.to_string()));
        }
        Ok(())
    }

    pub fn set_thread_provider_session(&self, id: &str, session_id: &str) -> Result<(), DbError> {
        let changed = self.conn().execute(
            "UPDATE threads SET provider_session_id = ?2 WHERE id = ?1",
            params![id, session_id],
        )?;
        if changed == 0 {
            return Err(DbError::ThreadNotFound(id.to_string()));
        }
        Ok(())
    }

    pub fn rename_thread(&self, id: &str, title: &str) -> Result<(), DbError> {
        let changed = self.conn().execute(
            "UPDATE threads SET title = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, title, now_millis()],
        )?;
        if changed == 0 {
            return Err(DbError::ThreadNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Remove a thread e, em cascata, seus eventos. Devolve se ela existia.
    pub fn delete_thread(&self, id: &str) -> Result<bool, DbError> {
        Ok(self.conn().execute("DELETE FROM threads WHERE id = ?1", [id])? > 0)
    }

    /// Acrescenta um evento com `seq` consecutivo e atualiza `updated_at`, atomicamente.
    pub fn append_event(
        &self,
        thread_id: &str,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Result<ThreadEvent, DbError> {
        let now = now_millis();
        let payload_text = serde_json::to_string(payload)?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM threads WHERE id = ?1)", [thread_id], |r| r.get(0))?;
        if !exists {
            return Err(DbError::ThreadNotFound(thread_id.to_string()));
        }
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM thread_events WHERE thread_id = ?1",
            [thread_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO thread_events (thread_id, seq, kind, payload, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![thread_id, seq, kind, payload_text, now],
        )?;
        tx.execute("UPDATE threads SET updated_at = ?2 WHERE id = ?1", params![thread_id, now])?;
        tx.commit()?;
        Ok(ThreadEvent { seq, kind: kind.to_string(), payload: payload.clone(), created_at: now })
    }

    /// Eventos de uma thread em ordem, a partir de `after_seq` (exclusivo).
    pub fn thread_events(&self, thread_id: &str, after_seq: i64) -> Result<Vec<ThreadEvent>, DbError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT seq, kind, payload, created_at FROM thread_events
             WHERE thread_id = ?1 AND seq > ?2 ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![thread_id, after_seq], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?))
        })?;
        rows.map(|r| {
            let (seq, kind, payload, created_at) = r?;
            Ok(ThreadEvent { seq, kind, payload: serde_json::from_str(&payload)?, created_at })
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn new_thread(provider: &str) -> NewThread {
        NewThread { provider: provider.to_string(), ..Default::default() }
    }

    #[test]
    fn test_create_and_get_thread() {
        let db = Database::open_in_memory().unwrap();
        let created = db
            .create_thread(NewThread {
                provider: "claude".into(),
                model: Some("opus".into()),
                title: Some("t".into()),
                cwd: Some("/tmp".into()),
            })
            .unwrap();
        assert_eq!(db.get_thread(&created.id).unwrap(), created);
        assert!(!created.pinned);
        assert!(matches!(db.get_thread("nope"), Err(DbError::ThreadNotFound(_))));
    }

    #[test]
    fn test_events_have_consecutive_seq_and_roundtrip_json() {
        let db = Database::open_in_memory().unwrap();
        let t = db.create_thread(new_thread("claude")).unwrap();
        let e1 = db.append_event(&t.id, "user", &json!({"text": "oi"})).unwrap();
        let e2 = db.append_event(&t.id, "assistant", &json!({"text": "olá", "n": [1, 2]})).unwrap();
        assert_eq!((e1.seq, e2.seq), (1, 2));

        let all = db.thread_events(&t.id, 0).unwrap();
        assert_eq!(all, vec![e1, e2.clone()]);
        assert_eq!(db.thread_events(&t.id, 1).unwrap(), vec![e2]);
    }

    #[test]
    fn test_append_to_missing_thread_fails() {
        let db = Database::open_in_memory().unwrap();
        assert!(matches!(
            db.append_event("ghost", "user", &json!({})),
            Err(DbError::ThreadNotFound(_))
        ));
    }

    #[test]
    fn test_append_bumps_updated_at_and_latest_thread() {
        let db = Database::open_in_memory().unwrap();
        let a = db.create_thread(new_thread("claude")).unwrap();
        let b = db.create_thread(new_thread("codex")).unwrap();
        assert_eq!(db.latest_thread().unwrap().unwrap().id, b.id);

        std::thread::sleep(std::time::Duration::from_millis(5));
        db.append_event(&a.id, "user", &json!({})).unwrap();
        assert_eq!(db.latest_thread().unwrap().unwrap().id, a.id);
    }

    #[test]
    fn test_pinned_threads_sort_first() {
        let db = Database::open_in_memory().unwrap();
        let a = db.create_thread(new_thread("claude")).unwrap();
        let b = db.create_thread(new_thread("codex")).unwrap();
        db.set_thread_pinned(&a.id, true).unwrap();
        let ids: Vec<_> = db.list_threads(10).unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![a.id, b.id]);
        assert!(matches!(db.set_thread_pinned("ghost", true), Err(DbError::ThreadNotFound(_))));
    }

    #[test]
    fn test_delete_cascades_to_events() {
        let db = Database::open_in_memory().unwrap();
        let t = db.create_thread(new_thread("claude")).unwrap();
        db.append_event(&t.id, "user", &json!({})).unwrap();
        assert!(db.delete_thread(&t.id).unwrap());
        assert!(!db.delete_thread(&t.id).unwrap());
        assert!(db.thread_events(&t.id, 0).unwrap().is_empty());
    }

    #[test]
    fn test_provider_session_id_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        let t = db.create_thread(new_thread("claude")).unwrap();
        assert_eq!(t.provider_session_id, None);
        db.set_thread_provider_session(&t.id, "abc").unwrap();
        assert_eq!(db.get_thread(&t.id).unwrap().provider_session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn test_rename_thread() {
        let db = Database::open_in_memory().unwrap();
        let t = db.create_thread(new_thread("claude")).unwrap();
        db.rename_thread(&t.id, "novo").unwrap();
        assert_eq!(db.get_thread(&t.id).unwrap().title.as_deref(), Some("novo"));
    }
}
