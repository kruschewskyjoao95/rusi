//! Migrations versionadas. A versão do banco é `PRAGMA user_version` e cada entrada
//! abaixo vale `índice + 1`. Nunca edite uma migration já publicada: acrescente outra.

pub(super) const MIGRATIONS: &[(&str, &str)] = &[
    (
        "001_threads_and_events",
        "
        CREATE TABLE threads (
            id         TEXT PRIMARY KEY,
            provider   TEXT NOT NULL,
            model      TEXT,
            title      TEXT,
            cwd        TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX idx_threads_updated ON threads(updated_at DESC);

        CREATE TABLE thread_events (
            thread_id  TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
            seq        INTEGER NOT NULL,
            kind       TEXT NOT NULL,
            payload    TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (thread_id, seq)
        );
        ",
    ),
    (
        "002_threads_pinned",
        "ALTER TABLE threads ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;",
    ),
    (
        "003_provider_accounts",
        "
        CREATE TABLE provider_accounts (
            provider   TEXT NOT NULL,
            account    TEXT NOT NULL,
            label      TEXT,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (provider, account)
        );
        ",
    ),
    (
        "004_threads_provider_session",
        "ALTER TABLE threads ADD COLUMN provider_session_id TEXT;",
    ),
    (
        "005_threads_worktree",
        "
        ALTER TABLE threads ADD COLUMN worktree_path TEXT;
        ALTER TABLE threads ADD COLUMN branch TEXT;
        ",
    ),
];
