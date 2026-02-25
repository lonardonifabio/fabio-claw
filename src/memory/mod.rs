use std::sync::Arc;
use tokio::sync::Mutex;
use rusqlite::{Connection, params};
use chrono::{DateTime, Utc};
use tracing::info;

use crate::errors::AppError;

pub struct ConversationEntry {
    pub session_id: String,
    pub user_message: String,
    pub assistant_message: String,
    pub model: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Clone)]
pub struct MemoryStore {
    conn: Arc<Mutex<Connection>>,
}

impl MemoryStore {
    pub fn open(db_path: &str) -> Result<Self, AppError> {
        let conn = Connection::open(db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        Self::migrate(&conn)?;
        info!(db_path = %db_path, "Memory store initialized");
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    fn migrate(conn: &Connection) -> Result<(), AppError> {
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS conversations (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id  TEXT NOT NULL,
                user_msg    TEXT NOT NULL,
                assistant_msg TEXT NOT NULL,
                model       TEXT NOT NULL,
                created_at  TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_session
                ON conversations(session_id);

            CREATE TABLE IF NOT EXISTS audit_log (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                event_type TEXT NOT NULL,
                payload    TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
        ")?;
        Ok(())
    }

    pub async fn save_conversation(&self, entry: ConversationEntry) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO conversations (session_id, user_msg, assistant_msg, model, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                entry.session_id,
                entry.user_message,
                entry.assistant_message,
                entry.model,
                entry.timestamp.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn get_session_history(
        &self,
        session_id: &str,
        limit: u32,
    ) -> Result<Vec<(String, String)>, AppError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT user_msg, assistant_msg FROM conversations
             WHERE session_id = ?1
             ORDER BY id DESC LIMIT ?2",
        )?;

        let rows = stmt
            .query_map(params![session_id, limit], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    #[allow(dead_code)]
    pub async fn log_audit(&self, event_type: &str, payload: Option<&str>) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO audit_log (event_type, payload) VALUES (?1, ?2)",
            params![event_type, payload],
        )?;
        Ok(())
    }

    pub async fn ping(&self) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        conn.execute_batch("SELECT 1")?;
        Ok(())
    }
}
