#!/bin/bash
# setup-new-plugins.sh — Creates all source files, builds, and installs 10 new plugins.
# Safe to re-run: overwrites source files, rebuilds, reinstalls.
#
#   cd ~/fabio-claw/plugins
#   bash setup-new-plugins.sh

set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_DIR="/opt/fabio-claw/plugins"

echo "🦀 Fabio-Claw Plugin Setup — v0.3.0"
echo "======================================"
echo "Working directory: $SCRIPT_DIR"

# ── Helper ────────────────────────────────────────────────────────────────────
write_file() {
    local path="$1"
    mkdir -p "$(dirname "$path")"
    cat > "$path"
    echo "  wrote $path"
}

# ══════════════════════════════════════════════════════════════════════════════
# 1. plugin-system-info
# ══════════════════════════════════════════════════════════════════════════════
echo ""
echo "▶ Creating plugin-system-info..."

write_file "$SCRIPT_DIR/plugin-system-info/Cargo.toml" << 'TOML'
[package]
name = "plugin-system-info"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "plugin-system-info"
path = "src/main.rs"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
TOML

mkdir -p "$SCRIPT_DIR/plugin-system-info/src"
write_file "$SCRIPT_DIR/plugin-system-info/src/main.rs" << 'RUST'
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};
use std::fs;

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
    let args = req.payload.get("args").and_then(|v| v.as_str()).unwrap_or("").trim().to_lowercase();
    let sub = if !args.is_empty() { args.as_str() } else { req.action.as_str() };
    match sub { "uptime" => get_uptime(), "disk" => get_disk(), _ => get_full_sysinfo() }
}

fn get_uptime() -> PluginResponse {
    match fs::read_to_string("/proc/uptime") {
        Ok(content) => {
            let secs = content.split_whitespace().next()
                .and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            let total = secs as u64;
            let days = total / 86400; let hours = (total % 86400) / 3600;
            let minutes = (total % 3600) / 60; let seconds = total % 60;
            let human = if days > 0 { format!("{}d {}h {}m {}s", days, hours, minutes, seconds) }
                        else if hours > 0 { format!("{}h {}m {}s", hours, minutes, seconds) }
                        else { format!("{}m {}s", minutes, seconds) };
            PluginResponse { success: true, result: serde_json::json!({
                "uptime_seconds": total, "uptime_human": human,
                "days": days, "hours": hours, "minutes": minutes }), error: None }
        }
        Err(e) => err(format!("Cannot read /proc/uptime: {}", e)),
    }
}

fn get_disk() -> PluginResponse {
    match std::process::Command::new("df").args(["-h", "/"]).output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let mut lines = stdout.lines(); let _hdr = lines.next();
            if let Some(line) = lines.next() {
                let c: Vec<&str> = line.split_whitespace().collect();
                if c.len() >= 6 {
                    return PluginResponse { success: true, result: serde_json::json!({
                        "filesystem": c[0], "size": c[1], "used": c[2],
                        "available": c[3], "use_pct": c[4], "mount": c[5] }), error: None };
                }
            }
            err("Could not parse df output".into())
        }
        Err(e) => err(format!("df failed: {}", e)),
    }
}

fn get_full_sysinfo() -> PluginResponse {
    let uptime = fs::read_to_string("/proc/uptime").ok()
        .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()))
        .map(|f| f as u64).unwrap_or(0);
    let days = uptime / 86400; let hours = (uptime % 86400) / 3600;
    let minutes = (uptime % 3600) / 60;
    let uptime_human = if days > 0 { format!("{}d {}h {}m", days, hours, minutes) }
                       else if hours > 0 { format!("{}h {}m", hours, minutes) }
                       else { format!("{}m", minutes) };

    let cpu_temp = ["/sys/class/thermal/thermal_zone0/temp",
                    "/sys/devices/virtual/thermal/thermal_zone0/temp"]
        .iter().find_map(|p| fs::read_to_string(p).ok()
            .and_then(|s| s.trim().parse::<f64>().ok()).map(|v| v / 1000.0));

    let mem = {
        let content = fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let mut total_kb = 0u64; let mut free_kb = 0u64; let mut avail_kb = 0u64;
        for line in content.lines() {
            let mut p = line.split_whitespace();
            match p.next() {
                Some("MemTotal:")     => { total_kb = p.next().and_then(|v| v.parse().ok()).unwrap_or(0); }
                Some("MemFree:")      => { free_kb  = p.next().and_then(|v| v.parse().ok()).unwrap_or(0); }
                Some("MemAvailable:") => { avail_kb = p.next().and_then(|v| v.parse().ok()).unwrap_or(0); }
                _ => {}
            }
        }
        let used = total_kb.saturating_sub(avail_kb);
        let pct = if total_kb > 0 { (used as f64 / total_kb as f64 * 100.0) as u32 } else { 0 };
        serde_json::json!({ "total_mb": total_kb/1024, "free_mb": free_kb/1024,
            "available_mb": avail_kb/1024, "used_mb": used/1024, "used_pct": pct })
    };

    let load_avg = {
        let c = fs::read_to_string("/proc/loadavg").unwrap_or_default();
        let p: Vec<&str> = c.split_whitespace().collect();
        serde_json::json!({ "1min":  p.first().copied().unwrap_or("?"),
            "5min":  p.get(1).copied().unwrap_or("?"),
            "15min": p.get(2).copied().unwrap_or("?") })
    };

    let disk = match std::process::Command::new("df").args(["-h", "/"]).output() {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out.stdout).to_string();
            let mut lines = s.lines(); let _h = lines.next();
            lines.next().map(|line| {
                let c: Vec<&str> = line.split_whitespace().collect();
                if c.len() >= 5 {
                    serde_json::json!({"size":c[1],"used":c[2],"available":c[3],"use_pct":c[4]})
                } else { serde_json::json!(null) }
            }).unwrap_or(serde_json::json!(null))
        }
        Err(_) => serde_json::json!(null),
    };

    PluginResponse { success: true, result: serde_json::json!({
        "uptime": uptime_human, "uptime_seconds": uptime,
        "cpu_temp_celsius": cpu_temp, "memory": mem,
        "load_avg": load_avg, "disk_root": disk
    }), error: None }
}

fn err(msg: String) -> PluginResponse {
    PluginResponse { success: false, result: Value::Null, error: Some(msg) }
}
RUST

write_file "$SCRIPT_DIR/plugin-system-info.json" << 'JSON'
{
  "name": "plugin-system-info",
  "version": "0.1.0",
  "description": "System info: CPU temp, RAM, uptime, disk. Usage: /sysinfo | /uptime | /disk",
  "commands": ["sysinfo", "uptime", "disk"],
  "default_action": "sysinfo",
  "payload_from_args": true,
  "require_signature": false,
  "tool_schema": {
    "description": "Return system metrics: CPU temperature, free RAM, uptime, and disk space.",
    "parameters": { "type": "object", "properties": {
      "args": { "type": "string", "description": "Sub-command: 'uptime', 'disk', or empty for full report" }
    }, "required": [] }
  }
}
JSON

# ══════════════════════════════════════════════════════════════════════════════
# 2. plugin-net-diagnostics
# ══════════════════════════════════════════════════════════════════════════════
echo ""
echo "▶ Creating plugin-net-diagnostics..."

write_file "$SCRIPT_DIR/plugin-net-diagnostics/Cargo.toml" << 'TOML'
[package]
name = "plugin-net-diagnostics"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "plugin-net-diagnostics"
path = "src/main.rs"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
TOML

mkdir -p "$SCRIPT_DIR/plugin-net-diagnostics/src"
write_file "$SCRIPT_DIR/plugin-net-diagnostics/src/main.rs" << 'RUST'
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};
use std::net::ToSocketAddrs;
use std::time::Instant;

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
    let host = if args.is_empty() { "8.8.8.8".to_string() } else { sanitize_host(&args) };
    match req.action.as_str() {
        "dns"     => do_dns(&host),
        "latency" => do_latency(&host),
        _         => do_ping(&host),
    }
}

fn sanitize_host(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
        .take(253).collect()
}

fn do_ping(host: &str) -> PluginResponse {
    match std::process::Command::new("ping").args(["-c", "4", "-W", "2", host]).output() {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout).to_string();
            let reachable = o.status.success();
            let rtt = stdout.lines()
                .find(|l| l.contains("min/avg/max") || l.contains("rtt"))
                .and_then(|l| l.split('=').nth(1)).map(|s| s.trim().to_string());
            let packet_loss = stdout.lines()
                .find(|l| l.contains("packet loss"))
                .and_then(|l| l.split(',').find(|p| p.contains("packet loss")))
                .map(|s| s.trim().to_string());
            PluginResponse { success: true, result: serde_json::json!({
                "host": host, "reachable": reachable,
                "packet_loss": packet_loss, "rtt_stats": rtt,
                "raw": &stdout[..stdout.len().min(400)] }), error: None }
        }
        Err(e) => err(format!("ping failed: {}", e)),
    }
}

fn do_dns(host: &str) -> PluginResponse {
    let t0 = Instant::now();
    match format!("{}:80", host).to_socket_addrs() {
        Ok(addrs) => {
            let elapsed_ms = t0.elapsed().as_millis() as u64;
            let resolved: Vec<String> = addrs.map(|a| a.ip().to_string()).collect();
            if resolved.is_empty() { return err(format!("No DNS records for '{}'", host)); }
            PluginResponse { success: true, result: serde_json::json!({
                "host": host, "resolved": resolved,
                "resolve_ms": elapsed_ms, "record_count": resolved.len() }), error: None }
        }
        Err(e) => err(format!("DNS failed for '{}': {}", host, e)),
    }
}

fn do_latency(host: &str) -> PluginResponse {
    let probes: Vec<Value> = [(443u16, "HTTPS"), (80, "HTTP"), (22, "SSH")].iter().map(|&(port, proto)| {
        let addr_str = format!("{}:{}", host, port);
        let t0 = Instant::now();
        let ok = addr_str.parse().ok().map(|a: std::net::SocketAddr| {
            std::net::TcpStream::connect_timeout(&a, std::time::Duration::from_millis(2000)).is_ok()
        }).unwrap_or(false);
        let ms = t0.elapsed().as_millis() as u64;
        serde_json::json!({ "protocol": proto, "port": port, "open": ok,
            "latency_ms": if ok { serde_json::json!(ms) } else { serde_json::json!(null) } })
    }).collect();
    PluginResponse { success: true,
        result: serde_json::json!({ "host": host, "probes": probes }), error: None }
}

fn err(msg: String) -> PluginResponse {
    PluginResponse { success: false, result: Value::Null, error: Some(msg) }
}
RUST

write_file "$SCRIPT_DIR/plugin-net-diagnostics.json" << 'JSON'
{
  "name": "plugin-net-diagnostics",
  "version": "0.1.0",
  "description": "Network diagnostics: ping, DNS, TCP latency. Usage: /ping <host> | /dns <host> | /latency <host>",
  "commands": ["ping", "dns", "latency"],
  "default_action": "ping",
  "payload_from_args": true,
  "require_signature": false,
  "tool_schema": {
    "description": "Run network diagnostics: ping a host, resolve DNS, or measure TCP latency.",
    "parameters": { "type": "object", "properties": {
      "args": { "type": "string", "description": "Target host or IP, e.g. '8.8.8.8' or 'google.com'" }
    }, "required": ["args"] }
  }
}
JSON

# ══════════════════════════════════════════════════════════════════════════════
# 3. plugin-log-tail
# ══════════════════════════════════════════════════════════════════════════════
echo ""
echo "▶ Creating plugin-log-tail..."

write_file "$SCRIPT_DIR/plugin-log-tail/Cargo.toml" << 'TOML'
[package]
name = "plugin-log-tail"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "plugin-log-tail"
path = "src/main.rs"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
TOML

mkdir -p "$SCRIPT_DIR/plugin-log-tail/src"
write_file "$SCRIPT_DIR/plugin-log-tail/src/main.rs" << 'RUST'
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};

#[derive(Debug, Deserialize)]
struct PluginRequest { action: String, payload: Value }
#[derive(Debug, Serialize)]
struct PluginResponse { success: bool, result: Value, error: Option<String> }

const MAX_LINES:     usize = 100;
const DEFAULT_LINES: usize = 20;

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
    let n = args.split_whitespace().find_map(|t| t.parse::<usize>().ok())
        .unwrap_or(DEFAULT_LINES).min(MAX_LINES);
    match req.action.as_str() {
        "errors" => fetch_logs(n, true),
        _        => fetch_logs(n, false),
    }
}

fn fetch_logs(n: usize, errors_only: bool) -> PluginResponse {
    let (lines, source) = fetch_via_journalctl(n, errors_only)
        .unwrap_or_else(|| fetch_via_logfile(n, errors_only));
    let sanitized: Vec<String> = lines.iter().map(|l| sanitize_line(l)).collect();
    let error_count = sanitized.iter().filter(|l| contains_error(l)).count();
    PluginResponse { success: true, result: serde_json::json!({
        "source": source, "lines_shown": sanitized.len(),
        "errors_only": errors_only, "error_count": error_count, "log": sanitized }), error: None }
}

fn fetch_via_journalctl(n: usize, errors_only: bool) -> Option<(Vec<String>, String)> {
    let mut cmd = std::process::Command::new("journalctl");
    cmd.args(["-u", "fabio-claw", "--no-pager", "-n", &n.to_string(), "--output=short"]);
    if errors_only { cmd.args(["-p", "err"]); }
    let out = cmd.output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    if lines.is_empty() { return None; }
    Some((lines, "journald:fabio-claw".to_string()))
}

fn fetch_via_logfile(n: usize, errors_only: bool) -> (Vec<String>, String) {
    for path in &["/var/log/syslog", "/var/log/messages", "/tmp/fabio-claw.log"] {
        if let Ok(content) = std::fs::read_to_string(path) {
            let all: Vec<String> = content.lines().map(|l| l.to_string()).collect();
            let filtered: Vec<String> = if errors_only {
                all.iter().filter(|l| contains_error(l)).cloned().collect()
            } else { all };
            let tail: Vec<String> = filtered.iter().rev().take(n).rev().cloned().collect();
            return (tail, path.to_string());
        }
    }
    (vec!["[No log source available. journald or syslog required.]".to_string()], "none".to_string())
}

fn sanitize_line(line: &str) -> String {
    let mut result = line.to_string();
    for kw in &["password", "secret", "token", "key", "auth", "credential"] {
        let lower = result.to_lowercase();
        if let Some(pos) = lower.find(kw) {
            let after = &result[pos + kw.len()..];
            if let Some(vs) = after.find(|c: char| c == '=' || c == ':') {
                let mf = pos + kw.len() + vs + 1;
                let mt = result.len().min(mf + 40);
                result = format!("{}[REDACTED]{}", &result[..mf], &result[mt..]);
            }
        }
    }
    if result.len() > 300 { result.truncate(297); result.push_str("..."); }
    result
}

fn contains_error(line: &str) -> bool {
    let l = line.to_lowercase();
    l.contains("error") || l.contains("fatal") || l.contains("panic")
        || l.contains("warn") || l.contains("crit")
}
RUST

write_file "$SCRIPT_DIR/plugin-log-tail.json" << 'JSON'
{
  "name": "plugin-log-tail",
  "version": "0.1.0",
  "description": "Tail application logs with secret sanitization. Usage: /logs [n] | /errors [n]",
  "commands": ["logs", "errors"],
  "default_action": "logs",
  "payload_from_args": true,
  "require_signature": false,
  "tool_schema": {
    "description": "Read the fabio-claw application log, optionally filtering to errors only.",
    "parameters": { "type": "object", "properties": {
      "args": { "type": "string", "description": "Number of lines to return (default 20), e.g. '50'" }
    }, "required": [] }
  }
}
JSON

# ══════════════════════════════════════════════════════════════════════════════
# 4. plugin-scheduler  ← FIXED: query_map match pattern
# ══════════════════════════════════════════════════════════════════════════════
echo ""
echo "▶ Creating plugin-scheduler (fixed)..."

write_file "$SCRIPT_DIR/plugin-scheduler/Cargo.toml" << 'TOML'
[package]
name = "plugin-scheduler"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "plugin-scheduler"
path = "src/main.rs"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
rusqlite = { version = "0.31", features = ["bundled"] }
chrono = "0.4"
TOML

mkdir -p "$SCRIPT_DIR/plugin-scheduler/src"
write_file "$SCRIPT_DIR/plugin-scheduler/src/main.rs" << 'RUST'
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
RUST

write_file "$SCRIPT_DIR/plugin-scheduler.json" << 'JSON'
{
  "name": "plugin-scheduler",
  "version": "0.1.0",
  "description": "Local reminders persisted in SQLite. Usage: /remind <time> <message> | /jobs",
  "commands": ["remind", "jobs"],
  "default_action": "remind",
  "payload_from_args": true,
  "require_signature": false,
  "tool_schema": {
    "description": "Create local reminders or list scheduled jobs. Time formats: 10min, 2h, 14:30, tomorrow.",
    "parameters": { "type": "object", "properties": {
      "args": { "type": "string", "description": "For /remind: '<time> <message>'. For /jobs: empty." }
    }, "required": [] }
  }
}
JSON

# ══════════════════════════════════════════════════════════════════════════════
# 5. plugin-rag-local  ← FIXED: query_map match pattern (4 occurrences)
# ══════════════════════════════════════════════════════════════════════════════
echo ""
echo "▶ Creating plugin-rag-local (fixed)..."

write_file "$SCRIPT_DIR/plugin-rag-local/Cargo.toml" << 'TOML'
[package]
name = "plugin-rag-local"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "plugin-rag-local"
path = "src/main.rs"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
rusqlite = { version = "0.31", features = ["bundled"] }
TOML

mkdir -p "$SCRIPT_DIR/plugin-rag-local/src"
write_file "$SCRIPT_DIR/plugin-rag-local/src/main.rs" << 'RUST'
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
RUST

write_file "$SCRIPT_DIR/plugin-rag-local.json" << 'JSON'
{
  "name": "plugin-rag-local",
  "version": "0.1.0",
  "description": "Local document search (RAG) via SQLite FTS5. Usage: /kb <query> | /search-doc <query>",
  "commands": ["kb", "search-doc"],
  "default_action": "search",
  "payload_from_args": true,
  "require_signature": false,
  "tool_schema": {
    "description": "Search local indexed documents using full-text search (BM25). Index with /kb index <path>.",
    "parameters": { "type": "object", "properties": {
      "args": { "type": "string", "description": "Search query, or 'index <path>' to index a directory" }
    }, "required": [] }
  }
}
JSON

# ══════════════════════════════════════════════════════════════════════════════
# 6. plugin-gpio-control
# ══════════════════════════════════════════════════════════════════════════════
echo ""
echo "▶ Creating plugin-gpio-control..."

write_file "$SCRIPT_DIR/plugin-gpio-control/Cargo.toml" << 'TOML'
[package]
name = "plugin-gpio-control"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "plugin-gpio-control"
path = "src/main.rs"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
TOML

mkdir -p "$SCRIPT_DIR/plugin-gpio-control/src"
write_file "$SCRIPT_DIR/plugin-gpio-control/src/main.rs" << 'RUST'
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct PluginRequest { action: String, payload: Value }
#[derive(Debug, Serialize)]
struct PluginResponse { success: bool, result: Value, error: Option<String> }

const SAFE_PINS: &[u8] = &[
    2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27
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
    if args.is_empty() || args == "status" || args == "all" { return gpio_status_all(); }
    let parts: Vec<&str> = args.splitn(2, char::is_whitespace).collect();
    match parts.as_slice() {
        [cmd, pin_str] => match pin_str.trim().parse::<u8>() {
            Ok(pin) => {
                if !SAFE_PINS.contains(&pin) {
                    return err(format!("Pin {} not in safe list {:?}", pin, SAFE_PINS));
                }
                match *cmd {
                    "on"  | "high" | "set"   => gpio_write(pin, true),
                    "off" | "low"  | "clear" => gpio_write(pin, false),
                    "read" | "get"           => gpio_read(pin),
                    other => err(format!("Unknown command '{}'. Use: on, off, read, status", other)),
                }
            }
            Err(_) => err(format!("Invalid pin number '{}'", pin_str.trim())),
        },
        [cmd] => match *cmd {
            "status" | "all" => gpio_status_all(),
            other => err(format!("Usage: /gpio <on|off|read> <pin>  Got: '{}'", other)),
        },
        _ => err("Usage: /gpio <on|off|read> <pin>".into()),
    }
}

fn gpio_export(pin: u8) -> Result<(), String> {
    let p = format!("/sys/class/gpio/gpio{}", pin);
    if !Path::new(&p).exists() {
        fs::write("/sys/class/gpio/export", pin.to_string()).map_err(|e|
            format!("Export pin {}: {} (add user to gpio group: sudo adduser $USER gpio)", pin, e))?;
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}

fn gpio_write(pin: u8, high: bool) -> PluginResponse {
    if let Err(e) = gpio_export(pin) { return err(e); }
    let base = format!("/sys/class/gpio/gpio{}", pin);
    if let Err(e) = fs::write(format!("{}/direction", base), "out") {
        return err(format!("Set direction pin {}: {}", pin, e));
    }
    match fs::write(format!("{}/value", base), if high { "1" } else { "0" }) {
        Ok(_) => PluginResponse { success: true, result: serde_json::json!({
            "pin":       pin,
            "state":     if high { "HIGH" } else { "LOW" },
            "value":     if high { 1 } else { 0 },
            "direction": "out"
        }), error: None },
        Err(e) => err(format!("Write pin {}: {}", pin, e)),
    }
}

fn gpio_read(pin: u8) -> PluginResponse {
    if let Err(e) = gpio_export(pin) { return err(e); }
    let base      = format!("/sys/class/gpio/gpio{}", pin);
    let direction = fs::read_to_string(format!("{}/direction", base))
        .unwrap_or_else(|_| "?".to_string()).trim().to_string();
    match fs::read_to_string(format!("{}/value", base)) {
        Ok(raw) => {
            let v: u8 = raw.trim().parse().unwrap_or(0);
            PluginResponse { success: true, result: serde_json::json!({
                "pin": pin, "value": v,
                "state": if v == 1 { "HIGH" } else { "LOW" },
                "direction": direction
            }), error: None }
        }
        Err(e) => err(format!("Read pin {}: {}", pin, e)),
    }
}

fn gpio_status_all() -> PluginResponse {
    let pins: Vec<Value> = SAFE_PINS.iter().filter_map(|&pin| {
        let p = format!("/sys/class/gpio/gpio{}", pin);
        if !Path::new(&p).exists() { return None; }
        let dir = fs::read_to_string(format!("{}/direction", p))
            .unwrap_or_default().trim().to_string();
        let v: u8 = fs::read_to_string(format!("{}/value", p))
            .unwrap_or_default().trim().parse().unwrap_or(0);
        Some(serde_json::json!({
            "pin": pin, "exported": true, "direction": dir,
            "value": v, "state": if v == 1 { "HIGH" } else { "LOW" }
        }))
    }).collect();
    PluginResponse { success: true, result: serde_json::json!({
        "exported_pins": pins.len(), "safe_pins": SAFE_PINS, "pins": pins,
        "note": "Only exported pins shown. Use /gpio on|off|read <pin> to interact."
    }), error: None }
}

fn err(msg: String) -> PluginResponse {
    PluginResponse { success: false, result: Value::Null, error: Some(msg) }
}
RUST

write_file "$SCRIPT_DIR/plugin-gpio-control.json" << 'JSON'
{
  "name": "plugin-gpio-control",
  "version": "0.1.0",
  "description": "Raspberry Pi GPIO via sysfs. Usage: /gpio on <pin> | /gpio off <pin> | /gpio read <pin>",
  "commands": ["gpio"],
  "default_action": "gpio",
  "payload_from_args": true,
  "require_signature": false,
  "tool_schema": {
    "description": "Control Raspberry Pi GPIO pins (BCM 2-27). Requires user in gpio group.",
    "parameters": { "type": "object", "properties": {
      "args": { "type": "string", "description": "'on 17', 'off 17', 'read 17', or 'status'" }
    }, "required": ["args"] }
  }
}
JSON

# ══════════════════════════════════════════════════════════════════════════════
# 7. plugin-updater
# ══════════════════════════════════════════════════════════════════════════════
echo ""
echo "▶ Creating plugin-updater..."

write_file "$SCRIPT_DIR/plugin-updater/Cargo.toml" << 'TOML'
[package]
name = "plugin-updater"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "plugin-updater"
path = "src/main.rs"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
TOML

mkdir -p "$SCRIPT_DIR/plugin-updater/src"
write_file "$SCRIPT_DIR/plugin-updater/src/main.rs" << 'RUST'
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};

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
    let args = req.payload.get("args").and_then(|v| v.as_str()).unwrap_or("").trim().to_lowercase();
    match req.action.as_str() {
        "plan" | "update-plan" => build_update_plan(&args),
        _                      => check_updates(&args),
    }
}

fn check_updates(target: &str) -> PluginResponse {
    let do_system  = target.is_empty() || target.contains("system") || target.contains("apt");
    let do_runtime = target.is_empty() || target.contains("fabio");
    let mut report = serde_json::json!({});
    if do_system  { report["system"]     = check_apt(); }
    if do_runtime { report["fabio_claw"] = check_runtime(); }
    report["checked_at"] = serde_json::json!(unix_now());
    report["safe_mode"]  = serde_json::json!(true);
    report["note"] = serde_json::json!("READ-ONLY check — no changes made. Use /update-plan for steps.");
    PluginResponse { success: true, result: report, error: None }
}

fn check_apt() -> Value {
    match std::process::Command::new("apt-get").args(["-s", "upgrade", "-q"]).output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let upgradable: Vec<String> = stdout.lines()
                .filter(|l| l.starts_with("Inst "))
                .map(|l| l[5..].split_whitespace().next().unwrap_or("").to_string())
                .filter(|s| !s.is_empty()).take(30).collect();
            serde_json::json!({
                "available_updates": upgradable.len(),
                "packages": &upgradable[..upgradable.len().min(20)],
                "status": if upgradable.is_empty() { "up_to_date" } else { "updates_available" }
            })
        }
        Err(e) => serde_json::json!({ "status": "check_failed", "error": format!("{}", e) }),
    }
}

fn check_runtime() -> Value {
    let git_log = std::process::Command::new("git")
        .args(["-C", "/home/pi/fabio-claw", "log", "--oneline", "-3"]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "git unavailable".to_string());
    serde_json::json!({
        "binary_path": "/usr/local/bin/fabio-claw",
        "git_recent_commits": git_log
    })
}

fn build_update_plan(target: &str) -> PluginResponse {
    let do_system  = target.is_empty() || target.contains("system") || target.contains("apt");
    let do_runtime = target.is_empty() || target.contains("fabio");
    let mut steps: Vec<Value> = vec![];
    if do_system {
        steps.push(serde_json::json!({
            "step": 1, "action": "Update system packages",
            "commands": ["sudo apt-get update", "sudo apt-get upgrade -y", "sudo apt-get autoremove -y"],
            "risk": "low", "requires_reboot": false
        }));
    }
    if do_runtime {
        let n = steps.len() + 1;
        steps.push(serde_json::json!({
            "step": n, "action": "Update fabio-claw runtime",
            "commands": ["cd ~/fabio-claw", "git pull origin main", "cargo build --release",
                "sudo cp target/release/fabio-claw /usr/local/bin/",
                "sudo systemctl restart fabio-claw"],
            "risk": "medium", "downtime_estimate": "30-60 minutes on Raspberry Pi"
        }));
        let n2 = steps.len() + 1;
        steps.push(serde_json::json!({
            "step": n2, "action": "Rebuild and reinstall plugins",
            "commands": ["cd ~/fabio-claw/plugins", "cargo build --release",
                "sudo cp target/release/plugin-* /opt/fabio-claw/plugins/",
                "sudo systemctl restart fabio-claw"],
            "risk": "low"
        }));
    }
    let nv = steps.len() + 1;
    steps.push(serde_json::json!({
        "step": nv, "action": "Verify health after update",
        "commands": ["curl http://localhost:8080/health", "curl http://localhost:8080/health/ready",
            "journalctl -u fabio-claw | tail -20"],
        "risk": "none"
    }));
    PluginResponse { success: true, result: serde_json::json!({
        "plan_generated_at": unix_now(), "total_steps": steps.len(), "steps": steps,
        "warning": "Review each step before executing. No commands have been run.",
        "safe_mode": true
    }), error: None }
}

fn unix_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("unix:{}", d.as_secs()))
        .unwrap_or_else(|_| "unknown".to_string())
}
RUST

write_file "$SCRIPT_DIR/plugin-updater.json" << 'JSON'
{
  "name": "plugin-updater",
  "version": "0.1.0",
  "description": "Safe update check and plan (read-only). Usage: /update-check | /update-plan",
  "commands": ["update-check", "update-plan"],
  "default_action": "check",
  "payload_from_args": true,
  "require_signature": false,
  "tool_schema": {
    "description": "Check for system/fabio-claw updates and generate a safe update plan. Never executes updates.",
    "parameters": { "type": "object", "properties": {
      "args": { "type": "string", "description": "Optional target: 'system', 'fabio-claw', or empty for both" }
    }, "required": [] }
  }
}
JSON

# ══════════════════════════════════════════════════════════════════════════════
# 8. plugin-rag-internet
# ══════════════════════════════════════════════════════════════════════════════
echo ""
echo "▶ Creating plugin-rag-internet..."

write_file "$SCRIPT_DIR/plugin-rag-internet/Cargo.toml" << 'TOML'
[package]
name = "plugin-rag-internet"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "plugin-rag-internet"
path = "src/main.rs"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
ureq = { version = "2", features = ["json"] }
TOML

mkdir -p "$SCRIPT_DIR/plugin-rag-internet/src"
write_file "$SCRIPT_DIR/plugin-rag-internet/src/main.rs" << 'RUST'
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};

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
    if args.is_empty() {
        return err("Provide a search query. Usage: /web-search <query>".into());
    }
    match req.action.as_str() {
        "rag" | "web-rag" => web_rag(&args),
        _                 => web_search(&args),
    }
}

fn web_search(query: &str) -> PluginResponse {
    let results = ddg_search(query);
    PluginResponse { success: true, result: serde_json::json!({
        "query": query, "source": "DuckDuckGo",
        "count": results.len(), "results": results
    }), error: None }
}

fn web_rag(query: &str) -> PluginResponse {
    let results = ddg_search(query);
    let synthesis = if results.is_empty() { "No results found.".to_string() } else {
        results.iter().take(3).enumerate().filter_map(|(i, r)| {
            let snippet = r["snippet"].as_str().unwrap_or("");
            let title   = r["title"].as_str().unwrap_or("");
            let url     = r["url"].as_str().unwrap_or("");
            if snippet.is_empty() { return None; }
            Some(format!("[{}] {} — {} ({})", i + 1, title, snippet, url))
        }).collect::<Vec<_>>().join("\n\n")
    };
    let citations: Vec<Value> = results.iter().map(|r| {
        serde_json::json!({ "title": r["title"], "url": r["url"] })
    }).collect();
    PluginResponse { success: true, result: serde_json::json!({
        "query": query, "synthesis": synthesis,
        "citations": citations, "source": "DuckDuckGo"
    }), error: None }
}

fn ddg_search(query: &str) -> Vec<Value> {
    let encoded = url_encode(query);
    let url = format!(
        "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1", encoded);
    let body: Value = match ureq::get(&url)
        .set("User-Agent", "fabio-claw/0.3.0").call() {
        Ok(r)  => match r.into_json() {
            Ok(j)  => j,
            Err(e) => return vec![serde_json::json!({"error": format!("{}", e)})],
        },
        Err(e) => return vec![serde_json::json!({
            "error": format!("Request failed: {}", e),
            "fallback_url": format!("https://duckduckgo.com/?q={}", encoded)
        })],
    };
    let mut results: Vec<Value> = vec![];
    if let Some(a) = body["Answer"].as_str() { if !a.is_empty() {
        results.push(serde_json::json!({
            "title": "Direct Answer", "snippet": a, "url": "", "type": "answer_box"
        }));
    }}
    if let Some(a) = body["Abstract"].as_str() { if !a.is_empty() {
        results.push(serde_json::json!({
            "title":   body["Heading"].as_str().unwrap_or("Summary"),
            "snippet": a,
            "url":     body["AbstractURL"].as_str().unwrap_or(""),
            "source":  body["AbstractSource"].as_str().unwrap_or(""),
            "type":    "abstract"
        }));
    }}
    if let Some(topics) = body["RelatedTopics"].as_array() {
        for t in topics.iter().take(5) {
            let text = t["Text"].as_str().unwrap_or_default();
            let url  = t["FirstURL"].as_str().unwrap_or_default();
            if !text.is_empty() {
                results.push(serde_json::json!({
                    "title":   &text[..text.len().min(80)],
                    "snippet": &text[..text.len().min(200)],
                    "url": url, "type": "related"
                }));
            }
        }
    }
    if results.is_empty() {
        results.push(serde_json::json!({
            "title":   "No instant results",
            "snippet": "Try a more specific query.",
            "url":     format!("https://duckduckgo.com/?q={}", encoded),
            "type":    "no_results"
        }));
    }
    results
}

fn url_encode(s: &str) -> String {
    s.chars().map(|c| {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') { c.to_string() }
        else if c == ' ' { "+".to_string() }
        else { format!("%{:02X}", c as u8) }
    }).collect()
}

fn err(msg: String) -> PluginResponse {
    PluginResponse { success: false, result: Value::Null, error: Some(msg) }
}
RUST

write_file "$SCRIPT_DIR/plugin-rag-internet.json" << 'JSON'
{
  "name": "plugin-rag-internet",
  "version": "0.1.0",
  "description": "Web search + RAG via DuckDuckGo (no API key). Usage: /web-search <q> | /web-rag <q>",
  "commands": ["web-search", "web-rag"],
  "default_action": "search",
  "payload_from_args": true,
  "require_signature": false,
  "tool_schema": {
    "description": "Search the web via DuckDuckGo and return ranked results with URL citations.",
    "parameters": { "type": "object", "properties": {
      "args": { "type": "string", "description": "Search query, e.g. 'Raspberry Pi GPIO pinout'" }
    }, "required": ["args"] }
  }
}
JSON

# ══════════════════════════════════════════════════════════════════════════════
# 9. plugin-word-builder (Python)
# ══════════════════════════════════════════════════════════════════════════════
echo ""
echo "▶ Creating plugin-word-builder (Python)..."

cat > "$SCRIPT_DIR/plugin-word-builder" << 'PYEOF'
#!/usr/bin/env python3
"""plugin-word-builder — generate .docx files.
Requires: pip install python-docx --break-system-packages
"""
import sys, json, os, re
from datetime import datetime
from pathlib import Path

OUTPUT_DIR   = "/var/lib/fabio-claw/documents"
FALLBACK_DIR = "/tmp/fabio-claw/documents"

TEMPLATES = {
    "meeting-minutes": {
        "title":    "Meeting Minutes",
        "sections": ["Attendees", "Agenda", "Discussion", "Action Items", "Next Meeting"]
    },
    "report": {
        "title":    "Technical Report",
        "sections": ["Executive Summary", "Introduction", "Analysis", "Conclusions", "Recommendations"]
    },
    "letter": {
        "title":    "Formal Letter",
        "sections": ["Recipient", "Subject", "Body", "Closing"]
    },
    "sop": {
        "title":    "Standard Operating Procedure",
        "sections": ["Purpose", "Scope", "Responsibilities", "Procedure", "References"]
    },
}

def main():
    raw = sys.stdin.read()
    try:
        req = json.loads(raw)
    except json.JSONDecodeError as e:
        print(json.dumps({"success": False, "result": None, "error": f"Invalid JSON: {e}"}))
        return
    action = req.get("action", "generate")
    args   = req.get("payload", {}).get("args", "").strip()
    try:
        if action in ("template", "doc-template"):
            result = handle_template(args)
        else:
            result = handle_generate(args)
        print(json.dumps({"success": True, "result": result, "error": None}))
    except ImportError:
        print(json.dumps({"success": False, "result": None,
            "error": "python-docx not installed. Run: pip install python-docx --break-system-packages"}))
    except Exception as e:
        print(json.dumps({"success": False, "result": None, "error": str(e)}))

def get_out():
    for d in [OUTPUT_DIR, FALLBACK_DIR, "/tmp"]:
        p = Path(d)
        try:
            p.mkdir(parents=True, exist_ok=True)
            return p
        except PermissionError:
            continue
    return Path("/tmp")

def handle_template(args):
    if not args:
        return {"available_templates": list(TEMPLATES.keys()),
                "usage": "/doc-template <n>  e.g. /doc-template meeting-minutes"}
    name = args.lower().replace(" ", "-")
    tpl  = TEMPLATES.get(name)
    if not tpl:
        return {"error": f"Template '{name}' not found.", "available": list(TEMPLATES.keys())}
    return make_from_template(name, tpl)

def handle_generate(args):
    if not args:
        return {"error": "Provide a description. Usage: /make-docx <description>",
                "templates": list(TEMPLATES.keys())}
    for name in TEMPLATES:
        if name in args.lower():
            return make_from_template(name, TEMPLATES[name])
    return make_freeform(args)

def make_from_template(name, tpl):
    from docx import Document
    from docx.shared import Pt, RGBColor
    from docx.enum.text import WD_ALIGN_PARAGRAPH
    doc = Document()
    now = datetime.now()
    safe  = re.sub(r'[^a-z0-9_-]', '', name)
    fname = f"{safe}_{now.strftime('%Y%m%d_%H%M%S')}.docx"
    out   = get_out() / fname
    tp = doc.add_heading(tpl["title"], 0)
    tp.alignment = WD_ALIGN_PARAGRAPH.CENTER
    m = doc.add_paragraph()
    m.alignment = WD_ALIGN_PARAGRAPH.CENTER
    r = m.add_run(f"Date: {now.strftime('%B %d, %Y')}")
    r.font.size      = Pt(10)
    r.font.color.rgb = RGBColor(0x66, 0x66, 0x66)
    doc.add_paragraph()
    for section in tpl["sections"]:
        doc.add_heading(section, level=1)
        doc.add_paragraph("[Fill in this section]")
        doc.add_paragraph()
    doc.save(str(out))
    return {"template": name, "file": str(out), "filename": fname,
            "sections": tpl["sections"], "created_at": now.isoformat()}

def make_freeform(prompt):
    from docx import Document
    from docx.shared import Pt, RGBColor
    from docx.enum.text import WD_ALIGN_PARAGRAPH
    doc   = Document()
    now   = datetime.now()
    title = prompt.split('.')[0].strip()[:60] or "Generated Document"
    safe  = re.sub(r'[^a-z0-9]', '_', title.lower())[:40]
    fname = f"doc_{safe}_{now.strftime('%Y%m%d_%H%M%S')}.docx"
    out   = get_out() / fname
    h = doc.add_heading(title, 0)
    h.alignment = WD_ALIGN_PARAGRAPH.CENTER
    m = doc.add_paragraph()
    m.alignment = WD_ALIGN_PARAGRAPH.CENTER
    r = m.add_run(f"Generated: {now.strftime('%B %d, %Y %H:%M')}")
    r.font.size      = Pt(10)
    r.font.color.rgb = RGBColor(0x88, 0x88, 0x88)
    doc.add_paragraph()
    for para in [p.strip() for p in prompt.split('\n') if p.strip()]:
        if para.endswith(':') and len(para) < 60:
            doc.add_heading(para.rstrip(':'), level=1)
        elif para.isupper() and len(para) < 60:
            doc.add_heading(para.title(), level=1)
        else:
            doc.add_paragraph(para)
    doc.save(str(out))
    return {"file": str(out), "filename": fname, "title": title,
            "created_at": now.isoformat()}

if __name__ == "__main__":
    main()
PYEOF
chmod +x "$SCRIPT_DIR/plugin-word-builder"

write_file "$SCRIPT_DIR/plugin-word-builder.json" << 'JSON'
{
  "name": "plugin-word-builder",
  "version": "0.1.0",
  "description": "Generate Word .docx from prompts or templates (requires python-docx). Usage: /make-docx <prompt> | /doc-template <n>",
  "commands": ["make-docx", "doc-template"],
  "default_action": "generate",
  "payload_from_args": true,
  "require_signature": false,
  "tool_schema": {
    "description": "Generate a Word document (.docx) from a text prompt or template name.",
    "parameters": { "type": "object", "properties": {
      "args": { "type": "string", "description": "Content prompt or template name" }
    }, "required": ["args"] }
  }
}
JSON

# ══════════════════════════════════════════════════════════════════════════════
# 10. plugin-excel-builder (Python)
# ══════════════════════════════════════════════════════════════════════════════
echo ""
echo "▶ Creating plugin-excel-builder (Python)..."

cat > "$SCRIPT_DIR/plugin-excel-builder" << 'PYEOF'
#!/usr/bin/env python3
"""plugin-excel-builder — generate .xlsx files.
Requires: pip install openpyxl --break-system-packages
"""
import sys, json, re
from datetime import datetime, timedelta
from pathlib import Path

OUTPUT_DIR   = "/var/lib/fabio-claw/documents"
FALLBACK_DIR = "/tmp/fabio-claw/documents"

TEMPLATES = {
    "budget":       "Monthly budget tracker with income, expenses and balance formulas",
    "inventory":    "Parts inventory with stock levels and LOW STOCK alert formulas",
    "sensor-log":   "IoT sensor data log with ALERT status formulas",
    "kpi":          "KPI dashboard with target vs actual metrics",
    "task-tracker": "Project task tracker with status and priority",
}

def main():
    raw = sys.stdin.read()
    try:
        req = json.loads(raw)
    except json.JSONDecodeError as e:
        print(json.dumps({"success": False, "result": None, "error": f"Invalid JSON: {e}"}))
        return
    action = req.get("action", "generate")
    args   = req.get("payload", {}).get("args", "").strip()
    try:
        if action in ("template", "sheet-template"):
            result = handle_template(args)
        else:
            result = handle_generate(args)
        print(json.dumps({"success": True, "result": result, "error": None}))
    except ImportError:
        print(json.dumps({"success": False, "result": None,
            "error": "openpyxl not installed. Run: pip install openpyxl --break-system-packages"}))
    except Exception as e:
        print(json.dumps({"success": False, "result": None, "error": str(e)}))

def get_out():
    for d in [OUTPUT_DIR, FALLBACK_DIR, "/tmp"]:
        p = Path(d)
        try:
            p.mkdir(parents=True, exist_ok=True)
            return p
        except PermissionError:
            continue
    return Path("/tmp")

def handle_template(args):
    if not args:
        return {"available_templates": list(TEMPLATES.keys()), "descriptions": TEMPLATES,
                "usage": "/sheet-template <n>  e.g. /sheet-template budget"}
    name = args.lower().strip()
    if name not in TEMPLATES:
        return {"error": f"Template '{name}' not found.", "available": list(TEMPLATES.keys())}
    return build_template(name)

def handle_generate(args):
    if not args:
        return {"error": "Provide a description. Usage: /make-xlsx <description>",
                "templates": list(TEMPLATES.keys())}
    for name in TEMPLATES:
        if name in args.lower():
            return build_template(name)
    return build_freeform(args)

def build_template(name):
    return {"budget": build_budget, "inventory": build_inventory,
            "sensor-log": build_sensor_log, "kpi": build_kpi,
            "task-tracker": build_task_tracker}[name]()

def hdr(ws, row, cols):
    from openpyxl.styles import Font, PatternFill, Alignment
    fill  = PatternFill("solid", fgColor="1F4E79")
    font  = Font(bold=True, color="FFFFFF", name="Arial", size=11)
    align = Alignment(horizontal="center", vertical="center")
    for c in range(1, cols + 1):
        cell = ws.cell(row=row, column=c)
        cell.fill = fill; cell.font = font; cell.alignment = align

def save(wb, name):
    now   = datetime.now()
    safe  = re.sub(r'[^a-z0-9_]', '_', name.lower())
    fname = f"{safe}_{now.strftime('%Y%m%d_%H%M%S')}.xlsx"
    out   = get_out() / fname
    wb.save(str(out))
    return {"file": str(out), "filename": fname, "template": name, "created_at": now.isoformat()}

def build_budget():
    from openpyxl import Workbook
    from openpyxl.styles import Font
    from openpyxl.utils import get_column_letter as gcl
    wb = Workbook(); ws = wb.active; ws.title = "Budget"
    now = datetime.now()
    ws["A1"] = "Monthly Budget Tracker"
    ws["A1"].font = Font(bold=True, size=16, name="Arial")
    ws["A2"] = f"Month: {now.strftime('%B %Y')}"
    ws["A4"] = "INCOME"
    ws["A4"].font = Font(bold=True, color="1F4E79", size=12, name="Arial")
    for i, h in enumerate(["Category", "Budgeted (€)", "Actual (€)", "Variance (€)"], 1):
        ws.cell(5, i).value = h
    hdr(ws, 5, 4)
    for i, cat in enumerate(["Salary", "Freelance", "Other Income"], 6):
        ws.cell(i,1).value=cat; ws.cell(i,2).value=0.0; ws.cell(i,3).value=0.0
        ws.cell(i,4).value=f"={gcl(3)}{i}-{gcl(2)}{i}"
    tr = 9
    ws.cell(tr,1).value="Total Income"; ws.cell(tr,1).font=Font(bold=True,name="Arial")
    for c in [2,3,4]:
        ws.cell(tr,c).value=f"=SUM({gcl(c)}6:{gcl(c)}{tr-1})"
        ws.cell(tr,c).number_format='#,##0.00'
    es = tr + 2
    ws.cell(es,1).value="EXPENSES"; ws.cell(es,1).font=Font(bold=True,color="C00000",size=12,name="Arial")
    for i, h in enumerate(["Category","Budgeted (€)","Actual (€)","Variance (€)"],1):
        ws.cell(es+1,i).value=h
    hdr(ws, es+1, 4)
    expense_cats=["Rent/Mortgage","Utilities","Groceries","Transport","Insurance","Entertainment","Other"]
    for i, cat in enumerate(expense_cats, es+2):
        ws.cell(i,1).value=cat; ws.cell(i,2).value=0.0; ws.cell(i,3).value=0.0
        ws.cell(i,4).value=f"={gcl(3)}{i}-{gcl(2)}{i}"
    et = es + 2 + len(expense_cats)
    ws.cell(et,1).value="Total Expenses"; ws.cell(et,1).font=Font(bold=True,name="Arial")
    for c in [2,3,4]:
        ws.cell(et,c).value=f"=SUM({gcl(c)}{es+2}:{gcl(c)}{et-1})"
        ws.cell(et,c).number_format='#,##0.00'
    nr = et + 2
    ws.cell(nr,1).value="NET BALANCE"; ws.cell(nr,1).font=Font(bold=True,size=12,name="Arial")
    for c in [2,3,4]:
        ws.cell(nr,c).value=f"={gcl(c)}{tr}-{gcl(c)}{et}"
        ws.cell(nr,c).number_format='#,##0.00'
    ws.column_dimensions["A"].width=22
    for col in ["B","C","D"]: ws.column_dimensions[col].width=16
    return save(wb,"budget")

def build_inventory():
    from openpyxl import Workbook
    from openpyxl.styles import Font
    from openpyxl.utils import get_column_letter as gcl
    wb = Workbook(); ws = wb.active; ws.title="Inventory"
    ws["A1"]="Parts Inventory"; ws["A1"].font=Font(bold=True,size=16,name="Arial")
    ws["A2"]=f"Last updated: {datetime.now().strftime('%Y-%m-%d %H:%M')}"
    hdrs=["ID","Part Name","Description","Qty","Min Stock","Unit","Location","Status"]
    for i,h in enumerate(hdrs,1): ws.cell(4,i).value=h
    hdr(ws,4,len(hdrs))
    rows=[("P001","Resistor 10kΩ","1/4W",250,50,"pcs","Drawer A1"),
          ("P002","LED Red 5mm","Standard",80,20,"pcs","Drawer A2"),
          ("P003","Raspberry Pi 4","4GB RAM",3,2,"pcs","Shelf B1"),
          ("P004","Micro SD 32GB","Class 10",1,3,"pcs","Shelf B2")]
    for i,row in enumerate(rows,5):
        for j,v in enumerate(row,1): ws.cell(i,j).value=v
        ws.cell(i,8).value=f'=IF(D{i}<E{i},"⚠ LOW STOCK","✓ OK")'
    for i,w in enumerate([8,20,25,8,10,8,15,14],1): ws.column_dimensions[gcl(i)].width=w
    return save(wb,"inventory")

def build_sensor_log():
    from openpyxl import Workbook
    from openpyxl.styles import Font
    from openpyxl.utils import get_column_letter as gcl
    wb = Workbook(); ws = wb.active; ws.title="Sensor Log"; now=datetime.now()
    ws["A1"]="IoT Sensor Data Log"; ws["A1"].font=Font(bold=True,size=16,name="Arial")
    hdrs=["Timestamp","Sensor ID","Sensor Name","Value","Unit","Min","Max","Status","Notes"]
    for i,h in enumerate(hdrs,1): ws.cell(3,i).value=h
    hdr(ws,3,len(hdrs))
    sensor_data=[("TEMP_01","CPU Temp",45.2,"°C",0,85),("HUM_01","Humidity",62.5,"%",20,80),
                 ("PRES_01","Pressure",1013.2,"hPa",900,1100),("VOLT_01","Voltage",5.02,"V",4.75,5.25)]
    for i,(sid,name,val,unit,lo,hi) in enumerate(sensor_data,4):
        ts=now-timedelta(minutes=i*5)
        ws.cell(i,1).value=ts.strftime("%Y-%m-%d %H:%M:%S"); ws.cell(i,2).value=sid
        ws.cell(i,3).value=name; ws.cell(i,4).value=val; ws.cell(i,5).value=unit
        ws.cell(i,6).value=lo; ws.cell(i,7).value=hi
        ws.cell(i,8).value=f'=IF(AND(D{i}>=F{i},D{i}<=G{i}),"✓ NORMAL","⚠ ALERT")'
    ws["A10"]="Average:"; ws["B10"]="=AVERAGE(D4:D7)"
    ws["A11"]="Alerts:";  ws["B11"]='=COUNTIF(H4:H7,"⚠ ALERT")'
    for i,w in enumerate([20,12,16,10,8,8,8,12,20],1): ws.column_dimensions[gcl(i)].width=w
    return save(wb,"sensor-log")

def build_kpi():
    from openpyxl import Workbook
    from openpyxl.styles import Font
    from openpyxl.utils import get_column_letter as gcl
    wb = Workbook(); ws = wb.active; ws.title="KPI Dashboard"
    ws["A1"]="KPI Dashboard"; ws["A1"].font=Font(bold=True,size=18,name="Arial")
    ws["A2"]=f"Period: {datetime.now().strftime('%B %Y')}"
    hdrs=["KPI","Target","Actual","Achievement %","Status","Notes"]
    for i,h in enumerate(hdrs,1): ws.cell(4,i).value=h
    hdr(ws,4,len(hdrs))
    kpis=[("System Uptime","99.9%","[fill]"),("Response Time (ms)","200","[fill]"),
          ("Requests/day","500","[fill]"),("Plugin Success Rate","98%","[fill]"),
          ("Error Rate","< 1%","[fill]")]
    for i,(name,target,actual) in enumerate(kpis,5):
        ws.cell(i,1).value=name; ws.cell(i,2).value=target; ws.cell(i,3).value=actual
    for i,w in enumerate([28,12,12,16,12,24],1): ws.column_dimensions[gcl(i)].width=w
    return save(wb,"kpi")

def build_task_tracker():
    from openpyxl import Workbook
    from openpyxl.styles import Font
    from openpyxl.utils import get_column_letter as gcl
    wb = Workbook(); ws = wb.active; ws.title="Tasks"
    ws["A1"]="Project Task Tracker"; ws["A1"].font=Font(bold=True,size=16,name="Arial")
    hdrs=["#","Task","Description","Priority","Status","Assignee","Due Date","Done %","Notes"]
    for i,h in enumerate(hdrs,1): ws.cell(3,i).value=h
    hdr(ws,3,len(hdrs))
    tasks=[(1,"Setup environment","Install dependencies","High","Done","pi","2024-01-15",100,""),
           (2,"Configure plugins","Test all plugins","High","In Progress","pi","2024-02-01",60,""),
           (3,"Deploy","Systemd service","Medium","Not Started","pi","2024-02-15",0,"")]
    for row in tasks: ws.append(row)
    for i,w in enumerate([4,22,28,10,14,12,12,8,20],1): ws.column_dimensions[gcl(i)].width=w
    return save(wb,"task-tracker")

def build_freeform(prompt):
    from openpyxl import Workbook
    from openpyxl.styles import Font
    from openpyxl.utils import get_column_letter as gcl
    wb = Workbook(); ws = wb.active; now = datetime.now()
    title = prompt.split('\n')[0][:60]
    ws.title = re.sub(r'[\\/*?:\[\]]','',title)[:31] or "Sheet1"
    ws["A1"]=title; ws["A1"].font=Font(bold=True,size=14,name="Arial")
    ws["A2"]=f"Generated: {now.strftime('%Y-%m-%d %H:%M')}"
    ws["A2"].font=Font(size=10,color="888888",name="Arial")
    for i,line in enumerate([l.strip() for l in prompt.split('\n') if l.strip()][1:],4):
        parts=(line.split('\t') if '\t' in line else (line.split(',') if ',' in line else [line]))
        for j,p in enumerate(parts,1): ws.cell(i,j).value=p.strip()
    for col in ws.columns:
        ml=max((len(str(c.value or "")) for c in col),default=10)
        ws.column_dimensions[gcl(col[0].column)].width=min(ml+2,40)
    safe=re.sub(r'[^a-z0-9]','_',title.lower())[:30]
    fname=f"sheet_{safe}_{now.strftime('%Y%m%d_%H%M%S')}.xlsx"
    out=get_out()/fname; wb.save(str(out))
    return {"file":str(out),"filename":fname,"title":title,"created_at":now.isoformat()}

if __name__ == "__main__":
    main()
PYEOF
chmod +x "$SCRIPT_DIR/plugin-excel-builder"

write_file "$SCRIPT_DIR/plugin-excel-builder.json" << 'JSON'
{
  "name": "plugin-excel-builder",
  "version": "0.1.0",
  "description": "Generate Excel .xlsx with formulas (requires openpyxl). Usage: /make-xlsx <desc> | /sheet-template <n>",
  "commands": ["make-xlsx", "sheet-template"],
  "default_action": "generate",
  "payload_from_args": true,
  "require_signature": false,
  "tool_schema": {
    "description": "Generate an Excel workbook (.xlsx) from a description or named template.",
    "parameters": { "type": "object", "properties": {
      "args": { "type": "string", "description": "Description or template: budget, inventory, sensor-log, kpi, task-tracker" }
    }, "required": ["args"] }
  }
}
JSON

# ══════════════════════════════════════════════════════════════════════════════
# Update workspace Cargo.toml
# ══════════════════════════════════════════════════════════════════════════════
echo ""
echo "▶ Updating workspace Cargo.toml..."

cat > "$SCRIPT_DIR/Cargo.toml" << 'TOML'
[workspace]
members = [
    "plugin-datetime",
    "plugin-calculator",
    "plugin-weather",
    "plugin-file-reader",
    # v0.3.0 plugins
    "plugin-system-info",
    "plugin-net-diagnostics",
    "plugin-log-tail",
    "plugin-scheduler",
    "plugin-rag-local",
    "plugin-gpio-control",
    "plugin-updater",
    "plugin-rag-internet",
]
resolver = "2"
TOML

# ══════════════════════════════════════════════════════════════════════════════
# Build all 8 Rust plugins
# ══════════════════════════════════════════════════════════════════════════════
echo ""
echo "▶ Building Rust plugins (10-20 min on Raspberry Pi, grab a coffee ☕)..."
cd "$SCRIPT_DIR"

cargo build --release \
    -p plugin-system-info \
    -p plugin-net-diagnostics \
    -p plugin-log-tail \
    -p plugin-scheduler \
    -p plugin-rag-local \
    -p plugin-gpio-control \
    -p plugin-updater \
    -p plugin-rag-internet

echo "✅ Rust build complete"

# ══════════════════════════════════════════════════════════════════════════════
# Install binaries, manifests, Python plugins
# ══════════════════════════════════════════════════════════════════════════════
echo ""
echo "▶ Installing to $PLUGIN_DIR..."
sudo mkdir -p "$PLUGIN_DIR"
sudo mkdir -p /var/lib/fabio-claw/documents
sudo mkdir -p /tmp/fabio-claw/documents

for plugin in \
    plugin-system-info \
    plugin-net-diagnostics \
    plugin-log-tail \
    plugin-scheduler \
    plugin-rag-local \
    plugin-gpio-control \
    plugin-updater \
    plugin-rag-internet; do
    sudo cp "$SCRIPT_DIR/target/release/$plugin" "$PLUGIN_DIR/"
    sudo cp "$SCRIPT_DIR/$plugin.json"           "$PLUGIN_DIR/"
    echo "  ✓ $plugin"
done

# Python dependencies
echo ""
echo "▶ Checking Python dependencies..."
if ! python3 -c "import docx" 2>/dev/null; then
    echo "  ⚠  Installing python-docx..."
    pip install python-docx --break-system-packages --quiet 2>/dev/null || \
        pip3 install python-docx --break-system-packages --quiet 2>/dev/null || true
fi
if ! python3 -c "import openpyxl" 2>/dev/null; then
    echo "  ⚠  Installing openpyxl..."
    pip install openpyxl --break-system-packages --quiet 2>/dev/null || \
        pip3 install openpyxl --break-system-packages --quiet 2>/dev/null || true
fi

sudo cp "$SCRIPT_DIR/plugin-word-builder"       "$PLUGIN_DIR/"
sudo cp "$SCRIPT_DIR/plugin-word-builder.json"  "$PLUGIN_DIR/"
sudo chmod +x "$PLUGIN_DIR/plugin-word-builder"
echo "  ✓ plugin-word-builder"

sudo cp "$SCRIPT_DIR/plugin-excel-builder"      "$PLUGIN_DIR/"
sudo cp "$SCRIPT_DIR/plugin-excel-builder.json" "$PLUGIN_DIR/"
sudo chmod +x "$PLUGIN_DIR/plugin-excel-builder"
echo "  ✓ plugin-excel-builder"

# ══════════════════════════════════════════════════════════════════════════════
# Restart and verify
# ══════════════════════════════════════════════════════════════════════════════
echo ""
echo "▶ Restarting fabio-claw..."
sudo systemctl restart fabio-claw
sleep 3

echo ""
echo "▶ Checking registered plugins:"
journalctl -u fabio-claw --no-pager -n 60 | grep -i "plugin\|registered\|loaded" || true

echo ""
echo "============================================================"
echo "✅ Setup complete! All 10 plugins installed."
echo ""
echo "Quick tests:"
echo "  /sysinfo                      — CPU temp, RAM, disk"
echo "  /ping 8.8.8.8                 — network ping"
echo "  /logs 20                      — last 20 log lines"
echo "  /remind 30min Check the Pi    — set a reminder"
echo "  /jobs                         — list reminders"
echo "  /kb                           — knowledge base index"
echo "  /gpio status                  — GPIO pin states"
echo "  /update-check                 — check for updates"
echo "  /web-search Raspberry Pi 5    — web search"
echo "  /make-docx project report     — generate Word doc"
echo "  /sheet-template budget        — generate Excel workbook"
echo "============================================================"
