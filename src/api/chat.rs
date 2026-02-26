use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use chrono::Utc;
use uuid::Uuid;
use tracing::{info, warn, instrument};

use crate::api::AppState;
use crate::errors::AppError;
use crate::llm::bpe_estimate;
use crate::memory::ConversationEntry;
use crate::plugins::{PluginRequest, PluginRunner};
use crate::agent::AgentRunner;

// ─── Request / Response types ────────────────────────────────────────────────

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
    /// Optional system prompt override (used for agent runs)
    pub system: Option<String>,
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

// ─── Handler ────────────────────────────────────────────────────────────────

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

    // ── Check if the last user message is a /command ───────────────────────
    if let Some((command, args)) = extract_command(&req.messages) {

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

        if let Some(manifest) = state.plugins.resolve(&command) {
            info!(plugin = %manifest.name, command = %command, "Dispatching to plugin");

            // Standardised payload: only {"args": "..."} — plugin handles the rest
            let payload = if manifest.payload_from_args && !args.is_empty() {
                serde_json::json!({ "args": args })
            } else {
                serde_json::json!({})
            };

            let plugin_req = PluginRequest {
                action: manifest.default_action.clone(),
                payload,
            };

            let manifest_clone = manifest.clone();
            let runner = PluginRunner::new(state.plugins.plugin_dir().to_string_lossy().as_ref());

            let content = match runner.run(&manifest_clone, &plugin_req, &state.device).await {
                Ok(r) if r.success => format_result(&manifest_clone.name, &r.result),
                Ok(r) => format!("⚠️ Plugin error: {}", r.error.unwrap_or_else(|| "unknown".into())),
                Err(e) => {
                    warn!(error = %e, plugin = %manifest_clone.name, "Plugin execution failed");
                    format!("⚠️ Plugin failed: {}", e)
                }
            };

            return ok_response(content, req.model, session_id, &state, &req.messages).await;
        }

        let content = format!(
            "⚠️ Unknown command `/{}`.\nType `/help` to see all available commands.",
            command
        );
        return ok_response(content, req.model, session_id, &state, &req.messages).await;
    }

    // ── Agent mode: ReAct loop ─────────────────────────────────────────────
    if state.llm.agent_mode() {
        let user_query = req.messages.last().map(|m| m.content.as_str()).unwrap_or("");
        let system = req.system.as_deref();
        let plugin_dir = state.plugins.plugin_dir().to_string_lossy().to_string();

        let agent = AgentRunner::new(
            state.llm.clone(),
            state.plugins.clone(),
            state.device.clone(),
            &plugin_dir,
        );

        let (answer, steps) = agent
            .run(user_query, system, req.max_tokens, req.temperature)
            .await?;

        // Include step trace as a debug comment in non-prod or as structured JSON
        let content = if std::env::var("AGENT_TRACE").map(|v| v == "1").unwrap_or(false) {
            format!(
                "{}\n\n---\n🔍 Agent trace ({} steps):\n{}",
                answer,
                steps.len(),
                serde_json::to_string_pretty(&steps).unwrap_or_default()
            )
        } else {
            answer
        };

        let prompt_tokens     = bpe_estimate(user_query);
        let completion_tokens = bpe_estimate(&content);
        let user_msg = req.messages.last().map(|m| m.content.clone()).unwrap_or_default();

        persist(&state, session_id, user_msg, content.clone(), req.model.clone()).await;

        return Ok(Json(ChatResponse {
            id: format!("chatcmpl-{}", Uuid::new_v4()),
            object: "chat.completion".into(),
            created: Utc::now().timestamp(),
            model: req.model,
            choices: vec![Choice {
                index: 0,
                message: ChatMessage { role: "assistant".into(), content },
                finish_reason: "stop".into(),
            }],
            usage: Usage {
                prompt_tokens,
                completion_tokens,
                total_tokens: prompt_tokens + completion_tokens,
            },
        }));
    }

    // ── Standard LLM inference ────────────────────────────────────────────
    let prompt = build_prompt(&req.messages);
    let infer_result = state.llm
        .infer_full(prompt.clone(), req.max_tokens, req.temperature)
        .await?;

    let user_msg = req.messages.last().map(|m| m.content.clone()).unwrap_or_default();
    persist(&state, session_id, user_msg, infer_result.text.clone(), req.model.clone()).await;

    Ok(Json(ChatResponse {
        id: format!("chatcmpl-{}", Uuid::new_v4()),
        object: "chat.completion".into(),
        created: Utc::now().timestamp(),
        model: req.model,
        choices: vec![Choice {
            index: 0,
            message: ChatMessage { role: "assistant".into(), content: infer_result.text },
            finish_reason: "stop".into(),
        }],
        usage: Usage {
            prompt_tokens: infer_result.prompt_tokens,
            completion_tokens: infer_result.completion_tokens,
            total_tokens: infer_result.prompt_tokens + infer_result.completion_tokens,
        },
    }))
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

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
    let prompt_tokens     = bpe_estimate(&content);
    let completion_tokens = prompt_tokens;
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
        usage: Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        },
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
