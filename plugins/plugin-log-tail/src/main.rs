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
