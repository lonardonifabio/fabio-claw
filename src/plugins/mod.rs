use std::path::{Path, PathBuf};
use std::time::Duration;
use std::process::{Command, Stdio};
use std::io::Write;
use serde::{Deserialize, Serialize};
use tracing::{info, warn, debug};

use crate::errors::AppError;
use crate::security::DeviceIdentity;

const PLUGIN_TIMEOUT_SECS: u64 = 10;

// ─── Manifest ────────────────────────────────────────────────────────────────

/// Each plugin ships a <name>.json manifest alongside its binary.
/// fabio-claw reads all manifests at startup and builds the routing table.
#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    /// Binary name (must match the executable in the plugins dir)
    pub name: String,
    pub version: String,
    pub description: String,
    /// Slash-commands this plugin handles, e.g. ["weather", "forecast", "meteo"]
    pub commands: Vec<String>,
    /// Which action string to send when the command is invoked
    pub default_action: String,
    /// If true, everything after the command is forwarded as {"args": "..."}
    /// If false, payload is always {}
    #[serde(default)]
    pub payload_from_args: bool,
}

// ─── Registry ────────────────────────────────────────────────────────────────

/// Loaded at startup; maps command → manifest.
/// Never changes at runtime — restart fabio-claw to pick up new plugins.
#[derive(Debug, Clone)]
pub struct PluginRegistry {
    /// command (lowercase) → manifest
    entries: std::collections::HashMap<String, PluginManifest>,
    plugin_dir: PathBuf,
}

impl PluginRegistry {
    /// Scan `plugin_dir` for *.json manifests and build the registry.
    pub fn load(plugin_dir: &str) -> Self {
        let dir = PathBuf::from(plugin_dir);
        let mut entries = std::collections::HashMap::new();

        let read = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) => {
                warn!(dir = %plugin_dir, error = %e, "Cannot read plugin directory");
                return Self { entries, plugin_dir: dir };
            }
        };

        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            match std::fs::read_to_string(&path) {
                Ok(text) => match serde_json::from_str::<PluginManifest>(&text) {
                    Ok(manifest) => {
                        // Check the binary exists alongside the manifest
                        let bin = dir.join(&manifest.name);
                        if !bin.exists() {
                            warn!(
                                manifest = %path.display(),
                                binary   = %bin.display(),
                                "Manifest found but binary missing — skipping"
                            );
                            continue;
                        }

                        info!(
                            plugin   = %manifest.name,
                            commands = ?manifest.commands,
                            "Registered plugin"
                        );

                        for cmd in &manifest.commands {
                            entries.insert(cmd.to_lowercase(), manifest.clone());
                        }
                    }
                    Err(e) => warn!(file = %path.display(), error = %e, "Invalid plugin manifest JSON"),
                },
                Err(e) => warn!(file = %path.display(), error = %e, "Cannot read plugin manifest"),
            }
        }

        info!(total_commands = entries.len(), "Plugin registry loaded");
        Self { entries, plugin_dir: dir }
    }

    /// Returns the manifest for a slash-command, if registered.
    pub fn resolve(&self, command: &str) -> Option<&PluginManifest> {
        self.entries.get(&command.to_lowercase())
    }

    /// List all registered commands (for /help or debug)
    pub fn commands(&self) -> Vec<(&str, &str)> {
        let mut list: Vec<(&str, &str)> = self.entries
            .iter()
            .map(|(cmd, m)| (cmd.as_str(), m.description.as_str()))
            .collect();
        list.sort_by_key(|(cmd, _)| *cmd);
        list
    }

    pub fn plugin_dir(&self) -> &Path {
        &self.plugin_dir
    }
}

// ─── Request / Response ──────────────────────────────────────────────────────

#[derive(Debug, Serialize, Clone)]
pub struct PluginRequest {
    pub action: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct PluginResponse {
    pub success: bool,
    pub result: serde_json::Value,
    pub error: Option<String>,
}

// ─── Runner ──────────────────────────────────────────────────────────────────

pub struct PluginRunner {
    plugin_dir: PathBuf,
}

impl PluginRunner {
    pub fn new(plugin_dir: String) -> Self {
        Self { plugin_dir: PathBuf::from(plugin_dir) }
    }

    pub fn run(
        &self,
        plugin_name: &str,
        request: &PluginRequest,
        _device: &DeviceIdentity,
    ) -> Result<PluginResponse, AppError> {
        let binary = self.plugin_dir.join(plugin_name);

        if !binary.exists() {
            return Err(AppError::PluginError(format!(
                "Plugin binary not found: {}",
                binary.display()
            )));
        }

        let input = serde_json::to_string(request)
            .map_err(|e| AppError::PluginError(format!("Serialize error: {}", e)))?;

        debug!(plugin = %plugin_name, input = %input, "Launching plugin");

        let mut child = Command::new(&binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| AppError::PluginError(format!("Failed to spawn '{}': {}", plugin_name, e)))?;

        // Write request to STDIN
        if let Some(stdin) = child.stdin.take() {
            let mut stdin = stdin;
            stdin.write_all(input.as_bytes())
                .map_err(|e| AppError::PluginError(format!("STDIN write error: {}", e)))?;
        }

        // Wait with timeout
        let deadline = std::time::Instant::now() + Duration::from_secs(PLUGIN_TIMEOUT_SECS);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if std::time::Instant::now() > deadline {
                        let _ = child.kill();
                        return Err(AppError::PluginError(format!(
                            "Plugin '{}' timed out after {}s",
                            plugin_name, PLUGIN_TIMEOUT_SECS
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(AppError::PluginError(format!("wait() error: {}", e))),
            }
        }

        let output = child.wait_with_output()
            .map_err(|e| AppError::PluginError(format!("Output read error: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str::<PluginResponse>(&stdout)
            .map_err(|e| AppError::PluginError(format!(
                "Plugin '{}' returned invalid JSON: {} | raw: {}",
                plugin_name, e, stdout.chars().take(200).collect::<String>()
            )))
    }
}
