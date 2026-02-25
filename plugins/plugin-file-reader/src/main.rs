use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct PluginRequest {
    action: String,
    payload: Value,
}

#[derive(Debug, Serialize)]
struct PluginResponse {
    success: bool,
    result: Value,
    error: Option<String>,
}

// Only allow reading from these safe directories
const ALLOWED_DIRS: &[&str] = &[
    "/home/pi/documents",
    "/home/pi/data",
    "/tmp/fabio-claw",
    "/var/lib/fabio-claw/data",
];

const MAX_FILE_SIZE: u64 = 512 * 1024; // 512KB max
const MAX_LINES: usize = 200;           // Max lines returned

fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap_or(0);

    let response = match serde_json::from_str::<PluginRequest>(&input) {
        Ok(req) => handle(req),
        Err(e) => PluginResponse {
            success: false,
            result: Value::Null,
            error: Some(format!("Invalid request JSON: {}", e)),
        },
    };

    println!("{}", serde_json::to_string(&response).unwrap());
}

fn handle(req: PluginRequest) -> PluginResponse {
    match req.action.as_str() {
        "read" => read_file(&req.payload),
        "list" => list_dir(&req.payload),
        "head" => head_file(&req.payload),
        "tail" => tail_file(&req.payload),
        _ => PluginResponse {
            success: false,
            result: Value::Null,
            error: Some(format!(
                "Unknown action '{}'. Supported: read, list, head, tail",
                req.action
            )),
        },
    }
}

fn is_path_allowed(path: &Path) -> Result<PathBuf, String> {
    // Resolve to absolute path (no symlink traversal)
    let canonical = path.canonicalize()
        .map_err(|e| format!("Cannot resolve path '{}': {}", path.display(), e))?;

    // Check against whitelist
    let allowed = ALLOWED_DIRS.iter().any(|dir| {
        canonical.starts_with(dir)
    });

    if !allowed {
        return Err(format!(
            "Access denied. Allowed directories: {}",
            ALLOWED_DIRS.join(", ")
        ));
    }

    Ok(canonical)
}

fn read_file(payload: &Value) -> PluginResponse {
    let path_str = match payload.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return err("Missing 'path' in payload".into()),
    };

    let path = match is_path_allowed(Path::new(path_str)) {
        Ok(p) => p,
        Err(e) => return err(e),
    };

    // Check file size before reading
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) => return err(format!("Cannot stat file: {}", e)),
    };

    if meta.len() > MAX_FILE_SIZE {
        return err(format!(
            "File too large ({} KB). Maximum is {} KB. Use 'head' or 'tail' for large files.",
            meta.len() / 1024,
            MAX_FILE_SIZE / 1024
        ));
    }

    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let truncated = lines.len() > MAX_LINES;
            let shown = &lines[..lines.len().min(MAX_LINES)];

            PluginResponse {
                success: true,
                result: serde_json::json!({
                    "path":       path.display().to_string(),
                    "size_bytes": meta.len(),
                    "lines":      lines.len(),
                    "truncated":  truncated,
                    "content":    shown.join("\n"),
                }),
                error: None,
            }
        }
        Err(e) => err(format!("Cannot read file: {}", e)),
    }
}

fn list_dir(payload: &Value) -> PluginResponse {
    let path_str = match payload.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return err("Missing 'path' in payload".into()),
    };

    let path = match is_path_allowed(Path::new(path_str)) {
        Ok(p) => p,
        Err(e) => return err(e),
    };

    match std::fs::read_dir(&path) {
        Ok(entries) => {
            let mut files = vec![];
            for entry in entries.flatten() {
                let meta = entry.metadata().ok();
                let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                let size   = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                files.push(serde_json::json!({
                    "name":   entry.file_name().to_string_lossy(),
                    "is_dir": is_dir,
                    "size":   size,
                }));
            }
            files.sort_by(|a, b| {
                a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or(""))
            });
            PluginResponse {
                success: true,
                result: serde_json::json!({
                    "path":  path.display().to_string(),
                    "count": files.len(),
                    "files": files,
                }),
                error: None,
            }
        }
        Err(e) => err(format!("Cannot list directory: {}", e)),
    }
}

fn head_file(payload: &Value) -> PluginResponse {
    let n = payload.get("lines").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    read_n_lines(payload, n, false)
}

fn tail_file(payload: &Value) -> PluginResponse {
    let n = payload.get("lines").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    read_n_lines(payload, n, true)
}

fn read_n_lines(payload: &Value, n: usize, from_end: bool) -> PluginResponse {
    let path_str = match payload.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return err("Missing 'path' in payload".into()),
    };

    let path = match is_path_allowed(Path::new(path_str)) {
        Ok(p) => p,
        Err(e) => return err(e),
    };

    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let all_lines: Vec<&str> = content.lines().collect();
            let total = all_lines.len();
            let selected: Vec<&str> = if from_end {
                let start = total.saturating_sub(n);
                all_lines[start..].to_vec()
            } else {
                all_lines[..n.min(total)].to_vec()
            };

            PluginResponse {
                success: true,
                result: serde_json::json!({
                    "path":        path.display().to_string(),
                    "total_lines": total,
                    "shown_lines": selected.len(),
                    "from_end":    from_end,
                    "content":     selected.join("\n"),
                }),
                error: None,
            }
        }
        Err(e) => err(format!("Cannot read file: {}", e)),
    }
}

fn err(msg: String) -> PluginResponse {
    PluginResponse { success: false, result: Value::Null, error: Some(msg) }
}
