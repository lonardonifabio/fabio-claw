use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use chrono::Utc;
use uuid::Uuid;
use tracing::{info, warn, instrument};

use crate::api::AppState;
use crate::errors::AppError;
use crate::memory::ConversationEntry;
use crate::plugins::{PluginRequest, PluginRunner};

// ─── Request / Response types ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default)]
    pub stream: bool,
    pub session_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

fn default_max_tokens() -> u32 { 512 }
fn default_temperature() -> f32 { 0.7 }

#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
}

#[derive(Debug, Serialize)]
pub struct Choice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: String,
}

#[derive(Debug, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

// ─── Handler ─────────────────────────────────────────────────────────────────

#[instrument(skip(state, req), fields(model = %req.model))]
pub async fn chat_completions(
    State(state): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, AppError> {
    if req.messages.is_empty() {
        return Err(AppError::InvalidRequest("messages cannot be empty".into()));
    }
    if req.stream {
        return Err(AppError::InvalidRequest(
            "Streaming not yet supported. Set stream=false.".into(),
        ));
    }

    let session_id = req.session_id.clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    info!(session_id = %session_id, "Processing chat request");

    // ── Check if the last user message is a /command ──────────────────────
    if let Some((command, args)) = extract_command(&req.messages) {

        // Special built-in: /help — lists all registered plugins
        if command == "help" {
            let lines: Vec<String> = state.plugins.commands()
                .iter()
                .map(|(cmd, desc)| format!("  /{:<20} {}", cmd, desc))
                .collect();
            let content = format!(
                "🦀 **Fabio-Claw — Available Commands**\n\n{}\n\n\
                 All other messages are sent to the LLM for inference.",
                lines.join("\n")
            );
            return ok_response(content, req.model, session_id, &state, &req.messages).await;
        }

        // Look up command in the plugin registry (fully dynamic — no hardcoding)
        if let Some(manifest) = state.plugins.resolve(&command) {
            info!(plugin = %manifest.name, command = %command, "Dispatching to plugin");

            let payload = if manifest.payload_from_args && !args.is_empty() {
                serde_json::json!({ "args": args, "city": args, "expression": args, "path": args })
            } else {
                serde_json::json!({})
            };

            let plugin_req = PluginRequest {
                action: manifest.default_action.clone(),
                payload,
            };

            let plugin_dir = state.plugins.plugin_dir().to_string_lossy().to_string();
            let runner = PluginRunner::new(plugin_dir);

            let content = match runner.run(&manifest.name, &plugin_req, &state.device) {
                Ok(r) if r.success => format_result(&manifest.name, &r.result),
                Ok(r) => format!("⚠️ Plugin error: {}", r.error.unwrap_or_else(|| "unknown".into())),
                Err(e) => {
                    warn!(error = %e, plugin = %manifest.name, "Plugin execution failed");
                    format!("⚠️ Plugin failed: {}", e)
                }
            };

            return ok_response(content, req.model, session_id, &state, &req.messages).await;
        }

        // Unknown command — helpful error
        let content = format!(
            "⚠️ Unknown command `/{}`.\nType `/help` to see all available commands.",
            command
        );
        return ok_response(content, req.model, session_id, &state, &req.messages).await;
    }

    // ── Standard LLM inference ────────────────────────────────────────────
    let prompt = build_prompt(&req.messages);
    let response_text = state.llm
        .infer(prompt.clone(), req.max_tokens, req.temperature)
        .await?;

    let prompt_tokens     = estimate_tokens(&prompt);
    let completion_tokens = estimate_tokens(&response_text);
    let user_msg = req.messages.last().map(|m| m.content.clone()).unwrap_or_default();

    persist(&state, session_id, user_msg, response_text.clone(), req.model.clone()).await;

    Ok(Json(ChatResponse {
        id: format!("chatcmpl-{}", Uuid::new_v4()),
        object: "chat.completion".into(),
        created: Utc::now().timestamp(),
        model: req.model,
        choices: vec![Choice {
            index: 0,
            message: ChatMessage { role: "assistant".into(), content: response_text },
            finish_reason: "stop".into(),
        }],
        usage: Usage { prompt_tokens, completion_tokens, total_tokens: prompt_tokens + completion_tokens },
    }))
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// If the last user message starts with '/', returns (command, rest_of_line).
fn extract_command(messages: &[ChatMessage]) -> Option<(String, String)> {
    let last = messages.iter().rev().find(|m| m.role == "user")?;
    let text = last.content.trim();
    if !text.starts_with('/') { return None; }
    let without_slash = &text[1..];
    let mut parts = without_slash.splitn(2, ' ');
    let cmd  = parts.next().unwrap_or("").to_lowercase();
    let args = parts.next().unwrap_or("").trim().to_string();
    if cmd.is_empty() { return None; }
    Some((cmd, args))
}

/// Format plugin JSON result into human-readable markdown.
/// Plugins that want rich formatting can ship a "format" hint in their manifest
/// (future). For now we pattern-match on known plugin names as a fallback,
/// and emit a generic pretty-print for unknown ones.
fn format_result(plugin_name: &str, result: &serde_json::Value) -> String {
    match plugin_name {
        "plugin-datetime" => format!(
            "🕐 **Date & Time**\n📅 Date: {}\n🕐 Time: {}\n📆 Day: {}\n🌍 Zone: {}",
            result["date"].as_str().unwrap_or("—"),
            result["time"].as_str().unwrap_or("—"),
            result["day_of_week"].as_str().unwrap_or("—"),
            result["timezone"].as_str().unwrap_or("—"),
        ),
        "plugin-weather" => {
            let mut out = format!(
                "🌍 **Weather — {}**\n🌤️ {}\n🌡️ Temp: {}\n🤔 Feels: {}\n💧 Humidity: {}\n💨 Wind: {}",
                result["location"].as_str().unwrap_or("—"),
                result["condition"].as_str().unwrap_or("—"),
                result["temperature"].as_str().unwrap_or("—"),
                result["feels_like"].as_str().unwrap_or("—"),
                result["humidity"].as_str().unwrap_or("—"),
                result["wind"].as_str().unwrap_or("—"),
            );
            if let Some(days) = result["forecast"].as_array() {
                out.push_str("\n\n📅 **3-Day Forecast**");
                for d in days {
                    out.push_str(&format!(
                        "\n  {} → max {:.0}°C / min {:.0}°C / rain {:.1}mm",
                        d["date"].as_str().unwrap_or(""),
                        d["max_temp"].as_f64().unwrap_or(0.0),
                        d["min_temp"].as_f64().unwrap_or(0.0),
                        d["rain_mm"].as_f64().unwrap_or(0.0),
                    ));
                }
            }
            out
        }
        "plugin-calculator" => format!(
            "🧮 **Calculator**\n📝 Expression: `{}`\n✅ Result: **{}**",
            result["expression"].as_str().unwrap_or("—"),
            result["result_str"].as_str().unwrap_or("—"),
        ),
        "plugin-file-reader" => format!(
            "📄 **File: {}**\n📏 Lines: {} | Size: {} bytes{}\n```\n{}\n```",
            result["path"].as_str().unwrap_or("—"),
            result["lines"].as_u64().or(result["total_lines"].as_u64()).unwrap_or(0),
            result["size_bytes"].as_u64().unwrap_or(0),
            if result["truncated"].as_bool().unwrap_or(false) { " (truncated)" } else { "" },
            result["content"].as_str().unwrap_or("(empty)"),
        ),
        // Generic fallback for future plugins — pretty-print the JSON
        _ => serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string()),
    }
}

async fn ok_response(
    content: String,
    model: String,
    session_id: String,
    state: &AppState,
    messages: &[ChatMessage],
) -> Result<Json<ChatResponse>, AppError> {
    let user_msg = messages.last().map(|m| m.content.clone()).unwrap_or_default();
    persist(state, session_id, user_msg, content.clone(), model.clone()).await;
    let t = estimate_tokens(&content);
    Ok(Json(ChatResponse {
        id: format!("chatcmpl-{}", Uuid::new_v4()),
        object: "chat.completion".into(),
        created: Utc::now().timestamp(),
        model,
        choices: vec![Choice {
            index: 0,
            message: ChatMessage { role: "assistant".into(), content },
            finish_reason: "stop".into(),
        }],
        usage: Usage { prompt_tokens: t, completion_tokens: t, total_tokens: t * 2 },
    }))
}

async fn persist(state: &AppState, session_id: String, user: String, assistant: String, model: String) {
    if let Err(e) = state.memory.save_conversation(ConversationEntry {
        session_id,
        user_message: user,
        assistant_message: assistant,
        model,
        timestamp: Utc::now(),
    }).await {
        warn!(error = %e, "Failed to persist conversation");
    }
}

fn build_prompt(messages: &[ChatMessage]) -> String {
    let mut p = String::new();
    for m in messages {
        match m.role.as_str() {
            "system"    => p.push_str(&format!("<|system|>\n{}\n", m.content)),
            "user"      => p.push_str(&format!("<|user|>\n{}\n", m.content)),
            "assistant" => p.push_str(&format!("<|assistant|>\n{}\n", m.content)),
            _           => p.push_str(&format!("{}: {}\n", m.role, m.content)),
        }
    }
    p.push_str("<|assistant|>\n");
    p
}

fn estimate_tokens(text: &str) -> u32 {
    (text.len() / 4).max(1) as u32
}
