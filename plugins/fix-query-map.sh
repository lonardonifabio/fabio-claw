#!/bin/bash
# fix-query-map.sh — patches the query_map type mismatch in plugin-scheduler and plugin-rag-local
# Run from: ~/fabio-claw/plugins
#   bash fix-query-map.sh

set -e
PLUGINS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
echo "📁 Working in: $PLUGINS_DIR"

# ── plugin-scheduler/src/main.rs ─────────────────────────────────────────────
echo "▶ Patching plugin-scheduler..."
mkdir -p "$PLUGINS_DIR/plugin-scheduler/src"
cat > "$PLUGINS_DIR/plugin-scheduler/src/main.rs" << 'RUST'
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, params};

#[derive(Debug, Deserialize)]
struct PluginRequest { action: String, payload: Value }
#[derive(Debug, Serialize)]
struct PluginResponse { success: bool, result: Value, error: Option<String> }

fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap_or(0);
    let response = match serde_json::from_str::<PluginRequest>(&input) {
        Ok(req) => handle(req),
        Err(e) => PluginResponse { success: false, result: Value::Null,
            error: Some(format!("Invalid request JSON: {}", e)) },
    };
    println!("{}", serde_json::to_string(&response).unwrap());
}

fn handle(req: PluginRequest) -> PluginResponse {
    let args = req.payload.get("args").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let db = match open_db() {
        Ok(c) => c,
        Err(e) => return PluginResponse { success: false, result: Value::Null,
            error: Some(format!("DB: {}", e)) },
    };
    match req.action.as_str() {
        "jobs" | "list" => list_reminders(&db),
        _ => if args.is_empty() { list_reminders(&db) } else { create_reminder(&db, &args) },
    }
}

fn open_db() -> rusqlite::Result<Connection> {
    let path = if std::path::Path::new("/var/lib/fabio-claw").exists() {
        "/var/lib/fabio-claw/reminders.db"
    } else {
        "/tmp/fabio-reminders.db"
    };
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS reminders (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            message    TEXT NOT NULL,
            remind_at  TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            done       INTEGER NOT NULL DEFAULT 0);"
    )?;
    Ok(conn)
}

fn create_reminder(db: &Connection, args: &str) -> PluginResponse {
    let parts: Vec<&str> = args.splitn(2, char::is_whitespace).collect();
    let (time_spec, message) = match parts.as_slice() {
        [t, m] => (t.to_string(), m.trim().to_string()),
        _ => return PluginResponse { success: false, result: Value::Null,
            error: Some("Usage: /remind <time> <message>  e.g. '/remind 30min Check the oven'".into()) },
    };
    if message.is_empty() {
        return PluginResponse { success: false, result: Value::Null,
            error: Some("Usage: /remind <time> <message>".into()) };
    }
    let remind_at = match parse_time_spec(&time_spec) {
        Some(t) => t,
        None => return PluginResponse { success: false, result: Value::Null,
            error: Some(format!("Cannot parse time '{}'. Try: 10min 2h 1h30m tomorrow 14:30", time_spec)) },
    };
    match db.execute("INSERT INTO reminders (message, remind_at) VALUES (?1, ?2)",
        params![message, remind_at.to_rfc3339()]) {
        Ok(_) => PluginResponse { success: true, result: serde_json::json!({
            "id":             db.last_insert_rowid(),
            "message":        message,
            "remind_at":      remind_at.to_rfc3339(),
            "remind_at_human": remind_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            "in_seconds":     (remind_at - Utc::now()).num_seconds()
        }), error: None },
        Err(e) => PluginResponse { success: false, result: Value::Null,
            error: Some(format!("Save failed: {}", e)) },
    }
}

fn list_reminders(db: &Connection) -> PluginResponse {
    let now = Utc::now();
    let _ = db.execute("UPDATE reminders SET done=1 WHERE done=0 AND remind_at <= ?1",
        params![now.to_rfc3339()]);

    let mut stmt = match db.prepare(
        "SELECT id, message, remind_at, done, created_at
         FROM reminders ORDER BY remind_at DESC LIMIT 20") {
        Ok(s)  => s,
        Err(e) => return PluginResponse { success: false, result: Value::Null,
            error: Some(format!("DB error: {}", e)) },
    };

    // FIX: match on query_map result instead of unwrap_or_else with Box
    let rows: Vec<Value> = match stmt.query_map([], |row| {
        Ok(serde_json::json!({
            "id":         row.get::<_, i64>(0)?,
            "message":    row.get::<_, String>(1)?,
            "remind_at":  row.get::<_, String>(2)?,
            "done":       row.get::<_, i64>(3)? != 0,
            "created_at": row.get::<_, String>(4)?
        }))
    }) {
        Ok(mapped) => mapped.flatten().collect(),
        Err(_)     => vec![],
    };

    let pending = rows.iter().filter(|r| r["done"] == false).count();
    PluginResponse { success: true, result: serde_json::json!({
        "total":     rows.len(),
        "pending":   pending,
        "done":      rows.len() - pending,
        "reminders": rows
    }), error: None }
}

fn parse_time_spec(spec: &str) -> Option<DateTime<Utc>> {
    let now = Utc::now();
    let s = spec.trim().to_lowercase();
    if let Some(d) = parse_relative_duration(&s) { return Some(now + d); }
    if let Some((h, m)) = parse_hhmm(&s) {
        let today = now.date_naive().and_hms_opt(h, m, 0)?;
        let c = DateTime::<Utc>::from_naive_utc_and_offset(today, Utc);
        return Some(if c > now { c } else { c + Duration::days(1) });
    }
    if s == "tomorrow" { return Some(now + Duration::days(1)); }
    None
}

fn parse_relative_duration(s: &str) -> Option<Duration> {
    let mut total = Duration::zero();
    let mut buf   = String::new();
    let mut found = false;
    for ch in s.chars() {
        if ch.is_ascii_digit() { buf.push(ch); } else {
            let n: i64 = buf.parse().unwrap_or(0); buf.clear();
            match ch {
                's' => { total = total + Duration::seconds(n); found = true; }
                'm' => { total = total + Duration::minutes(n); found = true; }
                'h' => { total = total + Duration::hours(n);   found = true; }
                'd' => { total = total + Duration::days(n);    found = true; }
                _ => {}
            }
        }
    }
    if found { Some(total) } else { None }
}

fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let p: Vec<&str> = s.split(':').collect();
    if p.len() == 2 {
        let h: u32 = p[0].parse().ok()?;
        let m: u32 = p[1].parse().ok()?;
        if h < 24 && m < 60 { return Some((h, m)); }
    }
    None
}
RUST
echo "  ✓ plugin-scheduler patched"

# ── plugin-rag-local/src/main.rs ─────────────────────────────────────────────
echo "▶ Patching plugin-rag-local..."
mkdir -p "$PLUGINS_DIR/plugin-rag-local/src"
cat > "$PLUGINS_DIR/plugin-rag-local/src/main.rs" << 'RUST'
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};
use std::path::Path;
use rusqlite::{Connection, params};

#[derive(Debug, Deserialize)]
struct PluginRequest { action: String, payload: Value }
#[derive(Debug, Serialize)]
struct PluginResponse { success: bool, result: Value, error: Option<String> }

const ALLOWED_DIRS: &[&str] = &[
    "/home/pi/documents", "/home/pi/manuals", "/home/pi/data",
    "/opt/fabio-claw/kb", "/tmp/fabio-claw/kb",
];

fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap_or(0);
    let response = match serde_json::from_str::<PluginRequest>(&input) {
        Ok(req) => handle(req),
        Err(e)  => PluginResponse { success: false, result: Value::Null,
            error: Some(format!("Invalid request JSON: {}", e)) },
    };
    println!("{}", serde_json::to_string(&response).unwrap());
}

fn handle(req: PluginRequest) -> PluginResponse {
    let args = req.payload.get("args").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let db = match open_db() {
        Ok(c)  => c,
        Err(e) => return err(format!("DB error: {}", e)),
    };
    match req.action.as_str() {
        "index" => index_path(&db, &args),
        "list"  => list_sources(&db),
        _ => if args.is_empty() { list_sources(&db) } else { search_kb(&db, &args) },
    }
}

fn open_db() -> rusqlite::Result<Connection> {
    let path = if Path::new("/var/lib/fabio-claw").exists() {
        "/var/lib/fabio-claw/rag-local.db"
    } else {
        "/tmp/fabio-rag-local.db"
    };
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS documents (
             id        INTEGER PRIMARY KEY AUTOINCREMENT,
             source    TEXT NOT NULL,
             chunk_idx INTEGER NOT NULL,
             content   TEXT NOT NULL,
             indexed_at TEXT NOT NULL DEFAULT (datetime('now')));
         CREATE UNIQUE INDEX IF NOT EXISTS idx_doc_chunk ON documents(source, chunk_idx);
         CREATE VIRTUAL TABLE IF NOT EXISTS docs_fts USING fts5(
             content, source UNINDEXED, chunk_idx UNINDEXED,
             content='documents', content_rowid='id');"
    )?;
    Ok(conn)
}

fn search_kb(db: &Connection, query: &str) -> PluginResponse {
    let safe: String = query.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '-')
        .collect();
    if safe.trim().is_empty() { return err("Empty query.".into()); }

    let fts_q = safe.split_whitespace()
        .map(|w| format!("\"{}\"", w))
        .collect::<Vec<_>>()
        .join(" OR ");

    let sql = "SELECT d.source, d.chunk_idx, d.content, bm25(docs_fts) as rank
               FROM docs_fts JOIN documents d ON docs_fts.rowid = d.id
               WHERE docs_fts MATCH ?1 ORDER BY rank LIMIT 5";

    // FIX: match on query_map result instead of unwrap_or_else with Box
    let results: Vec<Value> = match db.prepare(sql) {
        Ok(mut s) => match s.query_map(params![fts_q], |row| {
            Ok(serde_json::json!({
                "source":    row.get::<_, String>(0)?,
                "chunk_idx": row.get::<_, i64>(1)?,
                "excerpt":   trunc(&row.get::<_, String>(2)?, 300),
                "score":     row.get::<_, f64>(3).unwrap_or(0.0)
            }))
        }) {
            Ok(mapped) => mapped.flatten().collect(),
            Err(_)     => vec![],
        },
        Err(_) => vec![],
    };

    if results.is_empty() { return search_fallback(db, query); }

    PluginResponse { success: true, result: serde_json::json!({
        "query": query, "method": "fts5",
        "count": results.len(), "results": results
    }), error: None }
}

fn search_fallback(db: &Connection, query: &str) -> PluginResponse {
    let pat = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));

    // FIX: match on query_map result instead of unwrap_or_else with Box
    let results: Vec<Value> = match db.prepare(
        "SELECT source, chunk_idx, content FROM documents WHERE content LIKE ?1 LIMIT 5") {
        Ok(mut s) => match s.query_map(params![pat], |row| {
            Ok(serde_json::json!({
                "source":    row.get::<_, String>(0)?,
                "chunk_idx": row.get::<_, i64>(1)?,
                "excerpt":   trunc(&row.get::<_, String>(2)?, 300)
            }))
        }) {
            Ok(mapped) => mapped.flatten().collect(),
            Err(_)     => vec![],
        },
        Err(e) => return err(format!("Search error: {}", e)),
    };

    PluginResponse { success: true, result: serde_json::json!({
        "query":  query,
        "method": "like",
        "count":  results.len(),
        "results": results,
        "hint": if results.is_empty() {
            Some("No docs indexed yet. Use /kb index <path> to add documents.")
        } else { None }
    }), error: None }
}

fn index_path(db: &Connection, path_str: &str) -> PluginResponse {
    if path_str.is_empty() {
        return err(format!("Specify a path. Allowed dirs: {}", ALLOWED_DIRS.join(", ")));
    }
    let canonical = match Path::new(path_str).canonicalize() {
        Ok(p)  => p,
        Err(e) => return err(format!("Cannot resolve '{}': {}", path_str, e)),
    };
    if !ALLOWED_DIRS.iter().any(|d| canonical.starts_with(d)) {
        return err(format!("Access denied. Allowed: {}", ALLOWED_DIRS.join(", ")));
    }

    let mut indexed = 0usize;
    let mut errors: Vec<String> = vec![];

    if canonical.is_file() {
        match index_file(db, &canonical) {
            Ok(n)  => indexed += n,
            Err(e) => errors.push(e),
        }
    } else if canonical.is_dir() {
        for entry in std::fs::read_dir(&canonical).into_iter().flatten().flatten() {
            let p = entry.path();
            if p.is_file() {
                match index_file(db, &p) {
                    Ok(n)  => indexed += n,
                    Err(e) => errors.push(format!("{}: {}", p.display(), e)),
                }
            }
        }
    }

    PluginResponse { success: errors.is_empty(), result: serde_json::json!({
        "path": path_str, "chunks_indexed": indexed, "errors": errors
    }), error: None }
}

fn index_file(db: &Connection, path: &Path) -> Result<usize, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    if !matches!(ext.as_str(), "txt"|"md"|"rst"|"csv"|"log"|"json"|"yaml"|"toml") {
        return Err(format!("Unsupported file type: .{}", ext));
    }
    let content = std::fs::read_to_string(path).map_err(|e| format!("Read error: {}", e))?;
    let source  = path.to_string_lossy().to_string();

    let _ = db.execute("DELETE FROM documents WHERE source=?1", params![source]);

    let chunks = chunk_text(&content, 500, 50);
    let n = chunks.len();
    for (i, chunk) in chunks.iter().enumerate() {
        db.execute(
            "INSERT OR REPLACE INTO documents (source, chunk_idx, content) VALUES (?1, ?2, ?3)",
            params![source, i as i64, chunk],
        ).map_err(|e| format!("Insert: {}", e))?;
        let _ = db.execute(
            "INSERT INTO docs_fts(rowid, content, source, chunk_idx)
             VALUES (last_insert_rowid(), ?1, ?2, ?3)",
            params![chunk, source, i as i64],
        );
    }
    Ok(n)
}

fn chunk_text(text: &str, size: usize, overlap: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut chunks = vec![];
    let mut start  = 0;
    while start < chars.len() {
        let end = (start + size).min(chars.len());
        let c: String = chars[start..end].iter().collect();
        chunks.push(c.trim().to_string());
        if end >= chars.len() { break; }
        start += size - overlap;
    }
    chunks.into_iter().filter(|c| !c.is_empty()).collect()
}

fn list_sources(db: &Connection) -> PluginResponse {
    // FIX: match on query_map result instead of unwrap_or_else with Box
    let sources: Vec<Value> = match db.prepare(
        "SELECT source, COUNT(*) as chunks, MAX(indexed_at)
         FROM documents GROUP BY source ORDER BY 3 DESC LIMIT 20") {
        Ok(mut s) => match s.query_map([], |row| {
            Ok(serde_json::json!({
                "source":       row.get::<_, String>(0)?,
                "chunks":       row.get::<_, i64>(1)?,
                "last_indexed": row.get::<_, String>(2)?
            }))
        }) {
            Ok(mapped) => mapped.flatten().collect(),
            Err(_)     => vec![],
        },
        Err(e) => return err(format!("DB error: {}", e)),
    };

    PluginResponse { success: true, result: serde_json::json!({
        "indexed_sources": sources.len(),
        "sources":         sources,
        "allowed_dirs":    ALLOWED_DIRS,
        "tip": "Use /kb index <path> to add docs. Then /kb <query> to search."
    }), error: None }
}

fn trunc(s: &str, n: usize) -> String {
    if s.len() <= n { s.to_string() }
    else { let mut e = s[..n].to_string(); e.push('…'); e }
}

fn err(msg: String) -> PluginResponse {
    PluginResponse { success: false, result: Value::Null, error: Some(msg) }
}
RUST
echo "  ✓ plugin-rag-local patched"

# ── Rebuild only the two fixed plugins ───────────────────────────────────────
echo ""
echo "▶ Rebuilding patched plugins..."
cd "$PLUGINS_DIR"
cargo build --release -p plugin-scheduler -p plugin-rag-local

echo ""
echo "▶ Installing fixed binaries..."
sudo cp target/release/plugin-scheduler  /opt/fabio-claw/plugins/
sudo cp target/release/plugin-rag-local  /opt/fabio-claw/plugins/

echo ""
echo "▶ Restarting fabio-claw..."
sudo systemctl restart fabio-claw
sleep 2

echo ""
echo "============================================="
echo "✅ Fix complete! Quick tests:"
echo "  /remind 5min Test reminder"
echo "  /jobs"
echo "  /kb"
echo "============================================="
