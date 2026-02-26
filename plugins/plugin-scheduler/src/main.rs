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
        Err(e)  => PluginResponse { success: false, result: Value::Null,
            error: Some(format!("Invalid request JSON: {}", e)) },
    };
    println!("{}", serde_json::to_string(&response).unwrap());
}

fn handle(req: PluginRequest) -> PluginResponse {
    let args = req.payload.get("args").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let db = match open_db() {
        Ok(c)  => c,
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
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
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
            "id":              db.last_insert_rowid(),
            "message":         message,
            "remind_at":       remind_at.to_rfc3339(),
            "remind_at_human": remind_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            "in_seconds":      (remind_at - Utc::now()).num_seconds()
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

    // FIX: match on query_map — unwrap_or_else(Box::new(empty())) causes E0308 type mismatch
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
