use std::path::{Path, PathBuf};
use std::time::Duration;
use serde::{Deserialize, Serialize};
use tracing::{info, warn, debug, error};
use tokio::io::AsyncWriteExt;
use tokio::time::timeout;

use crate::errors::AppError;
use crate::security::DeviceIdentity;

const PLUGIN_TIMEOUT_SECS: u64 = 10;

// ─── Manifest ────────────────────────────────────────────────────────────────

/// Each plugin ships a `<name>.json` manifest alongside its binary (and a `<name>.sig`
/// for signature-verified deployments).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginManifest {
    /// Binary name (must match the executable in the plugins dir)
    pub name: String,
    pub version: String,
    pub description: String,
    /// Slash-commands this plugin handles, e.g. ["weather", "forecast", "meteo"]
    pub commands: Vec<String>,
    /// Which action string to send when the command is invoked
    pub default_action: String,
    /// If true, everything after the command is forwarded as `{"args": "..."}`.
    /// If false, payload is always `{}`.
    #[serde(default)]
    pub payload_from_args: bool,
    /// OpenAI-style JSON Schema for tool calling integration.
    /// If absent, a minimal schema is auto-generated from the manifest fields.
    #[serde(default)]
    pub tool_schema: Option<ToolSchema>,
    /// If true, require a valid `.sig` file before running this plugin.
    #[serde(default)]
    pub require_signature: bool,
}

/// OpenAI-compatible tool definition for agent tool-calling.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolSchema {
    pub description: String,
    pub parameters: serde_json::Value,
}

impl PluginManifest {
    /// Return an OpenAI-style tool definition for this plugin.
    pub fn as_tool_definition(&self) -> serde_json::Value {
        let schema = self.tool_schema.clone().unwrap_or_else(|| ToolSchema {
            description: self.description.clone(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "args": {
                        "type": "string",
                        "description": "Arguments to pass to the plugin"
                    }
                },
                "required": []
            }),
        });

        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name.replace('-', "_"),
                "description": schema.description,
                "parameters": schema.parameters
            }
        })
    }
}

// ─── Registry ────────────────────────────────────────────────────────────────

/// Loaded at startup; maps command → manifest.
/// Never changes at runtime — restart fabio-claw to pick up new plugins.
#[derive(Debug, Clone)]
pub struct PluginRegistry {
    /// command (lowercase) → manifest
    entries: std::collections::HashMap<String, PluginManifest>,
    /// plugin name → manifest (for tool-calling lookup by name)
    by_name: std::collections::HashMap<String, PluginManifest>,
    plugin_dir: PathBuf,
}

impl PluginRegistry {
    /// Scan `plugin_dir` for *.json manifests and build the registry.
    pub fn load(plugin_dir: &str) -> Self {
        let dir = PathBuf::from(plugin_dir);
        let mut entries  = std::collections::HashMap::new();
        let mut by_name  = std::collections::HashMap::new();

        let read = match std::fs::read_dir(&dir) {
            Ok(r)  => r,
            Err(e) => {
                warn!(dir = %plugin_dir, error = %e, "Cannot read plugin directory");
                return Self { entries, by_name, plugin_dir: dir };
            }
        };

        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }

            match std::fs::read_to_string(&path) {
                Ok(text) => match serde_json::from_str::<PluginManifest>(&text) {
                    Ok(manifest) => {
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
                        by_name.insert(manifest.name.clone(), manifest);
                    }
                    Err(e) => warn!(file = %path.display(), error = %e, "Invalid plugin manifest JSON"),
                },
                Err(e) => warn!(file = %path.display(), error = %e, "Cannot read plugin manifest"),
            }
        }

        info!(total_commands = entries.len(), "Plugin registry loaded");
        Self { entries, by_name, plugin_dir: dir }
    }

    /// Returns the manifest for a slash-command, if registered.
    pub fn resolve(&self, command: &str) -> Option<&PluginManifest> {
        self.entries.get(&command.to_lowercase())
    }

    /// Returns the manifest by plugin name (for tool-calling).
    pub fn resolve_by_name(&self, name: &str) -> Option<&PluginManifest> {
        self.by_name.get(name)
    }

    /// All registered commands (for /help or debug).
    pub fn commands(&self) -> Vec<(&str, &str)> {
        let mut list: Vec<(&str, &str)> = self.entries
            .iter()
            .map(|(cmd, m)| (cmd.as_str(), m.description.as_str()))
            .collect();
        list.sort_by_key(|(cmd, _)| *cmd);
        list
    }

    /// All plugins as OpenAI tool definitions (for agent tool-calling).
    pub fn as_tools(&self) -> Vec<serde_json::Value> {
        let mut seen = std::collections::HashSet::new();
        let mut tools = Vec::new();
        for manifest in self.by_name.values() {
            if seen.insert(manifest.name.clone()) {
                tools.push(manifest.as_tool_definition());
            }
        }
        tools.sort_by(|a, b| {
            a["function"]["name"].as_str().unwrap_or("")
                .cmp(b["function"]["name"].as_str().unwrap_or(""))
        });
        tools
    }

    pub fn plugin_dir(&self) -> &Path { &self.plugin_dir }
}

// ─── Request / Response ──────────────────────────────────────────────────────

/// Standardised request sent to every plugin over STDIN.
/// Plugins receive `payload.args` (a plain string) and handle their own
/// semantic parsing internally — no more field-guessing in the router.
#[derive(Debug, Serialize, Clone)]
pub struct PluginRequest {
    pub action: String,
    /// Always `{"args": "..."}` when `payload_from_args = true`, otherwise `{}`.
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
    pub fn new(plugin_dir: impl Into<PathBuf>) -> Self {
        Self { plugin_dir: plugin_dir.into() }
    }

    /// Run a plugin binary asynchronously with a hard timeout.
    ///
    /// Signature verification is performed BEFORE spawn when the manifest
    /// has `require_signature: true`.
    pub async fn run(
        &self,
        manifest: &PluginManifest,
        request: &PluginRequest,
        device: &DeviceIdentity,
    ) -> Result<PluginResponse, AppError> {
        let binary = self.plugin_dir.join(&manifest.name);

        if !binary.exists() {
            return Err(AppError::PluginError(format!(
                "Plugin binary not found: {}",
                binary.display()
            )));
        }

        // ── Signature verification ────────────────────────────────────────
        if manifest.require_signature {
            let sig_path = binary.with_extension("sig");
            if !sig_path.exists() {
                return Err(AppError::SecurityError(format!(
                    "Plugin '{}' requires a signature but '{}' is missing",
                    manifest.name,
                    sig_path.display()
                )));
            }
            device.verify_plugin_signature(&binary, &sig_path)?;
            debug!(plugin = %manifest.name, "Plugin signature verified");
        }

        let input = serde_json::to_string(request)
            .map_err(|e| AppError::PluginError(format!("Serialize error: {}", e)))?;

        debug!(plugin = %manifest.name, input = %input, "Launching plugin");

        // ── Async spawn with proper I/O (replaces polling loop) ───────────
        let mut child = tokio::process::Command::new(&binary)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| AppError::PluginError(format!(
                "Failed to spawn '{}': {}", manifest.name, e
            )))?;

        // Write request JSON to STDIN then close the pipe so the plugin sees EOF
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input.as_bytes()).await
                .map_err(|e| AppError::PluginError(format!("STDIN write error: {}", e)))?;
            // drop closes the pipe
        }

        // Wait with hard timeout
        let output = timeout(
            Duration::from_secs(PLUGIN_TIMEOUT_SECS),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| {
            error!(plugin = %manifest.name, "Plugin timed out");
            AppError::PluginError(format!(
                "Plugin '{}' timed out after {}s",
                manifest.name, PLUGIN_TIMEOUT_SECS
            ))
        })?
        .map_err(|e| AppError::PluginError(format!("wait_with_output error: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        serde_json::from_str::<PluginResponse>(&stdout)
            .map_err(|e| AppError::PluginError(format!(
                "Plugin '{}' returned invalid JSON: {} | raw: {}",
                manifest.name, e, stdout.chars().take(200).collect::<String>()
            )))
    }
}
