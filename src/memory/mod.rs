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

/// A stored document chunk for local RAG retrieval.
#[derive(Debug, Clone)]
pub struct RagChunk {
    pub id: Option<i64>,
    pub source: String,        // filename, URL, or label
    pub chunk_index: usize,
    pub content: String,
    /// Serialised f32 vector (little-endian bytes) — produced by the embedding worker.
    pub embedding: Vec<u8>,
    pub created_at: DateTime<Utc>,
}

/// A scheduled task persisted across restarts.
#[derive(Debug, Clone)]
pub struct ScheduledTask {
    pub id: Option<i64>,
    pub name: String,
    /// Cron expression, e.g. "0 * * * *" (every hour)
    pub cron_expr: String,
    /// Plugin name or slash-command to invoke
    pub command: String,
    /// Args forwarded to the plugin
    pub args: String,
    pub enabled: bool,
    pub last_run: Option<DateTime<Utc>>,
    pub next_run: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
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
            -- Conversation history
            CREATE TABLE IF NOT EXISTS conversations (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id      TEXT NOT NULL,
                user_msg        TEXT NOT NULL,
                assistant_msg   TEXT NOT NULL,
                model           TEXT NOT NULL,
                created_at      TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_session ON conversations(session_id);

            -- Audit log
            CREATE TABLE IF NOT EXISTS audit_log (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                event_type  TEXT NOT NULL,
                payload     TEXT,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );

            -- RAG document chunks
            CREATE TABLE IF NOT EXISTS rag_chunks (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                source      TEXT NOT NULL,
                chunk_index INTEGER NOT NULL,
                content     TEXT NOT NULL,
                embedding   BLOB NOT NULL,   -- raw f32 little-endian bytes
                created_at  TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_rag_source ON rag_chunks(source);

            -- Scheduled tasks
            CREATE TABLE IF NOT EXISTS scheduled_tasks (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                name        TEXT NOT NULL UNIQUE,
                cron_expr   TEXT NOT NULL,
                command     TEXT NOT NULL,
                args        TEXT NOT NULL DEFAULT '',
                enabled     INTEGER NOT NULL DEFAULT 1,
                last_run    TEXT,
                next_run    TEXT,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
        ")?;
        Ok(())
    }

    // ── Conversations ─────────────────────────────────────────────────────

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

    // ── RAG chunks ────────────────────────────────────────────────────────

    /// Store an embedded document chunk.
    pub async fn save_rag_chunk(&self, chunk: &RagChunk) -> Result<i64, AppError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO rag_chunks (source, chunk_index, content, embedding, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                chunk.source,
                chunk.chunk_index as i64,
                chunk.content,
                chunk.embedding,
                chunk.created_at.to_rfc3339(),
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Retrieve all chunks for a source document.
    pub async fn get_chunks_for_source(&self, source: &str) -> Result<Vec<RagChunk>, AppError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, source, chunk_index, content, embedding, created_at
             FROM rag_chunks WHERE source = ?1 ORDER BY chunk_index"
        )?;
        let rows = stmt.query_map(params![source], |row| {
            Ok(RagChunk {
                id: Some(row.get(0)?),
                source: row.get(1)?,
                chunk_index: row.get::<_, i64>(2)? as usize,
                content: row.get(3)?,
                embedding: row.get(4)?,
                created_at: row.get::<_, String>(5)?
                    .parse::<DateTime<Utc>>()
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Retrieve the top-k most similar chunks to a query embedding using cosine similarity.
    /// The embedding is a slice of f32 stored as little-endian bytes in the DB.
    pub async fn search_rag(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<(f32, RagChunk)>, AppError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, source, chunk_index, content, embedding, created_at FROM rag_chunks"
        )?;

        let rows: Vec<RagChunk> = stmt.query_map([], |row| {
            Ok(RagChunk {
                id: Some(row.get(0)?),
                source: row.get(1)?,
                chunk_index: row.get::<_, i64>(2)? as usize,
                content: row.get(3)?,
                embedding: row.get(4)?,
                created_at: row.get::<_, String>(5)?
                    .parse::<DateTime<Utc>>()
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?.collect::<Result<Vec<_>, _>>()?;

        let mut scored: Vec<(f32, RagChunk)> = rows
            .into_iter()
            .filter_map(|chunk| {
                let stored: Vec<f32> = chunk.embedding
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();
                let score = cosine_similarity(query_embedding, &stored);
                Some((score, chunk))
            })
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored)
    }

    pub async fn delete_rag_source(&self, source: &str) -> Result<usize, AppError> {
        let conn = self.conn.lock().await;
        let n = conn.execute("DELETE FROM rag_chunks WHERE source = ?1", params![source])?;
        Ok(n)
    }

    // ── Scheduled tasks ───────────────────────────────────────────────────

    pub async fn upsert_task(&self, task: &ScheduledTask) -> Result<i64, AppError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO scheduled_tasks (name, cron_expr, command, args, enabled, next_run)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(name) DO UPDATE SET
               cron_expr = excluded.cron_expr,
               command   = excluded.command,
               args      = excluded.args,
               enabled   = excluded.enabled,
               next_run  = excluded.next_run",
            params![
                task.name,
                task.cron_expr,
                task.command,
                task.args,
                task.enabled as i64,
                task.next_run.map(|t| t.to_rfc3339()),
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub async fn list_tasks(&self) -> Result<Vec<ScheduledTask>, AppError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, name, cron_expr, command, args, enabled, last_run, next_run, created_at
             FROM scheduled_tasks ORDER BY name"
        )?;
        let rows = stmt.query_map([], |row| {
            let parse_opt = |s: Option<String>| {
                s.and_then(|v| v.parse::<DateTime<Utc>>().ok())
            };
            Ok(ScheduledTask {
                id: Some(row.get(0)?),
                name: row.get(1)?,
                cron_expr: row.get(2)?,
                command: row.get(3)?,
                args: row.get(4)?,
                enabled: row.get::<_, i64>(5)? != 0,
                last_run: parse_opt(row.get(6)?),
                next_run: parse_opt(row.get(7)?),
                created_at: row.get::<_, String>(8)?
                    .parse::<DateTime<Utc>>()
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub async fn update_task_run_times(
        &self,
        name: &str,
        last_run: DateTime<Utc>,
        next_run: Option<DateTime<Utc>>,
    ) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE scheduled_tasks SET last_run = ?1, next_run = ?2 WHERE name = ?3",
            params![last_run.to_rfc3339(), next_run.map(|t| t.to_rfc3339()), name],
        )?;
        Ok(())
    }

    pub async fn delete_task(&self, name: &str) -> Result<(), AppError> {
        let conn = self.conn.lock().await;
        conn.execute("DELETE FROM scheduled_tasks WHERE name = ?1", params![name])?;
        Ok(())
    }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() { return 0.0; }
    let dot: f32  = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32   = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32   = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 { return 0.0; }
    dot / (na * nb)
}
