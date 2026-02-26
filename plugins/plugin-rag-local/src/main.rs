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
             id         INTEGER PRIMARY KEY AUTOINCREMENT,
             source     TEXT NOT NULL,
             chunk_idx  INTEGER NOT NULL,
             content    TEXT NOT NULL,
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

    // FIX: match on query_map result
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

    // FIX: match on query_map result
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
    // FIX: match on query_map result
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
