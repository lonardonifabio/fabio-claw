// src/plugin_manager.rs
//
// Improvements over the original approach:
//   1. Per-plugin subprocess timeout (prevents hung Pi)
//   2. Response cache for deterministic plugins (sysinfo, datetime)
//   3. Plugin health check at startup
//   4. Structured error types instead of raw strings
//   5. Plugin registry with O(1) command lookup

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};
use std::io::Write;
use serde::{Deserialize, Serialize};

// ─── Config ───────────────────────────────────────────────────────────────────
const PLUGIN_DIR:        &str = "/opt/fabio-claw/plugins";
const PLUGIN_TIMEOUT_MS: u64  = 8_000;   // 8s max per plugin call
const CACHE_TTL_SECS:    u64  = 10;      // cache sysinfo/datetime for 10s
const CACHE_TTL_WEATHER: u64  = 300;     // cache weather for 5 minutes

// ─── Types ────────────────────────────────────────────────────────────────────
#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    pub name:             String,
    pub version:          String,
    pub description:      String,
    pub commands:         Vec<String>,
    pub default_action:   String,
    #[serde(default = "default_true")]
    pub payload_from_args: bool,
    #[serde(default)]
    pub cacheable:        bool,
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl_secs:   u64,
}

fn default_true()      -> bool { true  }
fn default_cache_ttl() -> u64  { CACHE_TTL_SECS }

#[derive(Debug, Clone)]
pub struct RegisteredPlugin {
    pub manifest:    PluginManifest,
    pub binary_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRequest {
    pub action:  String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginResponse {
    pub success: bool,
    pub result:  serde_json::Value,
    pub error:   Option<String>,
}

impl PluginResponse {
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            result:  serde_json::Value::Null,
            error:   Some(msg.into()),
        }
    }
}

#[derive(Debug)]
pub enum PluginError {
    NotFound(String),
    Timeout(String),
    SpawnFailed(String),
    InvalidResponse(String),
    IoError(String),
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginError::NotFound(cmd)       => write!(f, "No plugin handles /{}", cmd),
            PluginError::Timeout(name)       => write!(f, "Plugin {} timed out after {}s", name, PLUGIN_TIMEOUT_MS/1000),
            PluginError::SpawnFailed(msg)    => write!(f, "Plugin launch failed: {}", msg),
            PluginError::InvalidResponse(m)  => write!(f, "Plugin returned invalid response: {}", m),
            PluginError::IoError(msg)        => write!(f, "Plugin I/O error: {}", msg),
        }
    }
}

// ─── Cache entry ─────────────────────────────────────────────────────────────
struct CacheEntry {
    response:   PluginResponse,
    created_at: Instant,
    ttl:        Duration,
}

impl CacheEntry {
    fn is_fresh(&self) -> bool {
        self.created_at.elapsed() < self.ttl
    }
}

// ─── Plugin Manager ───────────────────────────────────────────────────────────
pub struct PluginManager {
    /// command → plugin
    registry: HashMap<String, RegisteredPlugin>,
    /// "plugin_name:args_key" → cached response
    cache:    HashMap<String, CacheEntry>,
}

impl PluginManager {
    /// Scan plugin directory, load manifests, build command registry.
    pub fn new() -> Self {
        let mut mgr = Self {
            registry: HashMap::new(),
            cache:    HashMap::new(),
        };
        mgr.scan_plugins();
        mgr
    }

    fn scan_plugins(&mut self) {
        let dir = Path::new(PLUGIN_DIR);
        if !dir.exists() {
            eprintln!("[PluginManager] Plugin dir {} not found", PLUGIN_DIR);
            return;
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(e)  => e,
            Err(e) => { eprintln!("[PluginManager] Cannot read {}: {}", PLUGIN_DIR, e); return; }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let json_str = match std::fs::read_to_string(&path) {
                Ok(s)  => s,
                Err(e) => { eprintln!("[PluginManager] Cannot read {:?}: {}", path, e); continue; }
            };

            let manifest: PluginManifest = match serde_json::from_str(&json_str) {
                Ok(m)  => m,
                Err(e) => { eprintln!("[PluginManager] Invalid manifest {:?}: {}", path, e); continue; }
            };

            let binary = dir.join(&manifest.name);
            if !binary.exists() {
                eprintln!("[PluginManager] Binary {:?} not found, skipping", binary);
                continue;
            }

            // Check execute permission
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&binary) {
                    if meta.permissions().mode() & 0o111 == 0 {
                        eprintln!("[PluginManager] {:?} not executable, skipping", binary);
                        continue;
                    }
                }
            }

            let plugin = RegisteredPlugin {
                binary_path: binary,
                manifest:    manifest.clone(),
            };

            for cmd in &manifest.commands {
                self.registry.insert(cmd.clone(), plugin.clone());
            }

            println!("[PluginManager] Registered plugin={} commands={:?}",
                     manifest.name, manifest.commands);
        }
    }

    /// Returns list of all registered (command, description) pairs for /help.
    pub fn list_commands(&self) -> Vec<(String, String)> {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut result: Vec<(String, String)> = self.registry.iter()
            .filter_map(|(cmd, plugin)| {
                if seen.insert(cmd.clone()) {
                    Some((cmd.clone(), plugin.manifest.description.clone()))
                } else {
                    None
                }
            })
            .collect();
        result.sort_by(|a, b| a.0.cmp(&b.0));
        result
    }

    /// Find plugin for a command. Returns None if not found.
    pub fn find(&self, command: &str) -> Option<&RegisteredPlugin> {
        self.registry.get(command)
    }

    /// Execute a plugin call with:
    ///   - cache check (for cacheable plugins)
    ///   - subprocess timeout
    ///   - structured error handling
    pub fn call(
        &mut self,
        command: &str,
        args:    &str,
    ) -> Result<PluginResponse, PluginError> {
        let plugin = self.registry.get(command)
            .ok_or_else(|| PluginError::NotFound(command.to_string()))?
            .clone();

        // ── Cache check ───────────────────────────────────────────────────────
        let cache_key = format!("{}:{}", plugin.manifest.name, args);
        if plugin.manifest.cacheable {
            if let Some(entry) = self.cache.get(&cache_key) {
                if entry.is_fresh() {
                    return Ok(entry.response.clone());
                }
            }
        }

        // ── Build request ─────────────────────────────────────────────────────
        let payload = if plugin.manifest.payload_from_args {
            serde_json::json!({ "args": args })
        } else {
            serde_json::json!({})
        };

        // Determine action: some commands map to a different action
        let action = self.resolve_action(command, &plugin);

        let req = PluginRequest {
            action,
            payload,
        };

        let req_json = serde_json::to_string(&req)
            .map_err(|e| PluginError::IoError(e.to_string()))?;

        // ── Spawn subprocess with timeout ─────────────────────────────────────
        let response = self.run_plugin_with_timeout(&plugin, &req_json)?;

        // ── Cache the result ──────────────────────────────────────────────────
        if plugin.manifest.cacheable && response.success {
            let ttl = Duration::from_secs(
                cache_ttl_for(&plugin.manifest.name, args)
            );
            self.cache.insert(cache_key, CacheEntry {
                response: response.clone(),
                created_at: Instant::now(),
                ttl,
            });
        }

        Ok(response)
    }

    fn resolve_action(&self, command: &str, plugin: &RegisteredPlugin) -> String {
        // Some commands within the same plugin need different action strings
        // e.g., plugin-scheduler handles both "remind" and "jobs" commands
        match (plugin.manifest.name.as_str(), command) {
            ("plugin-scheduler",     "jobs")          => "jobs".into(),
            ("plugin-scheduler",     "remind")        => "remind".into(),
            ("plugin-rag-local",     "search-doc")    => "search".into(),
            ("plugin-rag-internet",  "web-rag")       => "rag".into(),
            ("plugin-system-info",   "uptime")        => "uptime".into(),
            ("plugin-system-info",   "disk")          => "disk".into(),
            ("plugin-log-tail",      "errors")        => "errors".into(),
            ("plugin-updater",       "update-plan")   => "plan".into(),
            ("plugin-word-builder",  "doc-template")  => "template".into(),
            ("plugin-excel-builder", "sheet-template") => "template".into(),
            _ => plugin.manifest.default_action.clone(),
        }
    }

    fn run_plugin_with_timeout(
        &self,
        plugin:   &RegisteredPlugin,
        req_json: &str,
    ) -> Result<PluginResponse, PluginError> {
        // Spawn the child process
        let mut child = Command::new(&plugin.binary_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())  // suppress plugin stderr from user output
            .spawn()
            .map_err(|e| PluginError::SpawnFailed(format!("{}: {}", plugin.manifest.name, e)))?;

        // Write request to stdin
        if let Some(stdin) = child.stdin.take() {
            let mut stdin = stdin;
            stdin.write_all(req_json.as_bytes())
                .map_err(|e| PluginError::IoError(e.to_string()))?;
            // stdin dropped here → EOF sent to child
        }

        // Wait with timeout using a thread
        let timeout = Duration::from_millis(PLUGIN_TIMEOUT_MS);
        let start   = Instant::now();

        // Poll every 50ms for completion
        loop {
            match child.try_wait() {
                Ok(Some(_status)) => break,
                Ok(None) => {
                    if start.elapsed() > timeout {
                        let _ = child.kill();
                        return Err(PluginError::Timeout(plugin.manifest.name.clone()));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    return Err(PluginError::IoError(e.to_string()));
                }
            }
        }

        // Read stdout
        let output = child.wait_with_output()
            .map_err(|e| PluginError::IoError(e.to_string()))?;

        let raw = String::from_utf8_lossy(&output.stdout);
        let raw = raw.trim();

        if raw.is_empty() {
            return Err(PluginError::InvalidResponse("empty response".into()));
        }

        serde_json::from_str::<PluginResponse>(raw)
            .map_err(|e| PluginError::InvalidResponse(format!("{} — raw: {}", e, &raw[..raw.len().min(120)])))
    }

    /// Validate all registered plugins by doing a dry-run health check.
    /// Returns list of (plugin_name, ok, message).
    pub fn health_check(&mut self) -> Vec<(String, bool, String)> {
        let mut results = Vec::new();
        let mut seen = std::collections::HashSet::new();

        let plugins: Vec<(String, RegisteredPlugin)> = self.registry.iter()
            .map(|(_, p)| (p.manifest.name.clone(), p.clone()))
            .filter(|(name, _)| seen.insert(name.clone()))
            .collect();

        for (name, plugin) in plugins {
            let binary = &plugin.binary_path;
            if !binary.exists() {
                results.push((name, false, "binary not found".into()));
                continue;
            }

            // Quick smoke test: send a minimal valid request
            let req = r#"{"action":"__healthcheck__","payload":{"args":""}}"#;
            match self.run_plugin_with_timeout(&plugin, req) {
                Ok(_)  => results.push((name, true, "ok".into())),
                Err(PluginError::Timeout(n)) =>
                    results.push((n, false, "timed out on healthcheck".into())),
                Err(e) => {
                    // Most plugins return "unknown action" for __healthcheck__
                    // which is still a valid response → plugin is alive
                    let msg = e.to_string();
                    let alive = msg.contains("Unknown action") || msg.contains("unknown action")
                        || msg.contains("Unsupported") || msg.contains("Invalid");
                    results.push((name, alive, if alive { "alive".into() } else { msg }));
                }
            }
        }

        results
    }
}

/// Determine cache TTL based on plugin name and args.
fn cache_ttl_for(plugin_name: &str, _args: &str) -> u64 {
    match plugin_name {
        "plugin-datetime"    => 5,                // time changes every second, keep short
        "plugin-system-info" => CACHE_TTL_SECS,   // 10s
        "plugin-weather"     => CACHE_TTL_WEATHER, // 5 min
        _                    => 0,                // no cache
    }
}

// ─── Help formatter ───────────────────────────────────────────────────────────
pub fn format_help(commands: &[(String, String)]) -> String {
    if commands.is_empty() {
        return "No plugins registered. Check /opt/fabio-claw/plugins/".into();
    }
    let max_cmd = commands.iter().map(|(c, _)| c.len()).max().unwrap_or(10);
    let lines: Vec<String> = commands.iter()
        .map(|(cmd, desc)| format!("  /{:<width$}  {}", cmd, desc, width = max_cmd))
        .collect();
    format!("Available commands:\n{}", lines.join("\n"))
}
