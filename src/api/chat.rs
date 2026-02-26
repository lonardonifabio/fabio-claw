// src/api/chat.rs
//
// Rewritten chat handler integrating:
//   1. PluginManager (timeout, cache, health)
//   2. PromptEngine (TinyLlama ChatML format, summarisation, cleaning)
//   3. ConversationHistory (per-session sliding window)
//   4. Structured error responses
//   5. /help and /reset built-in commands

use std::sync::{Arc, Mutex};
use axum::{extract::State, Json, http::StatusCode};
use serde::{Deserialize, Serialize};

use crate::plugin_manager::{PluginManager, PluginError, format_help};
use crate::prompt_engine::{
    build_prompt, clean_response, summarise_plugin_result,
    parse_slash_command, ConversationHistory, Role,
};
use crate::llm_client::LlmClient;

// ─── Request / Response types ─────────────────────────────────────────────────
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub model:      String,
    pub messages:   Vec<ChatMessage>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default)]
    pub stream:     bool,
}

fn default_max_tokens() -> u32 { 256 }

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ChatMessage {
    pub role:    String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub id:      String,
    pub object:  String,
    pub created: u64,
    pub model:   String,
    pub choices: Vec<ChatChoice>,
    pub usage:   TokenUsage,
}

#[derive(Debug, Serialize)]
pub struct ChatChoice {
    pub index:         u32,
    pub message:       ChatMessage,
    pub finish_reason: String,
}

#[derive(Debug, Serialize)]
pub struct TokenUsage {
    pub prompt_tokens:     u32,
    pub completion_tokens: u32,
    pub total_tokens:      u32,
}

// ─── App state ────────────────────────────────────────────────────────────────
pub struct AppState {
    pub plugin_manager: Arc<Mutex<PluginManager>>,
    pub llm_client:     Arc<LlmClient>,
    pub history:        Arc<Mutex<ConversationHistory>>,
}

// ─── Handler ──────────────────────────────────────────────────────────────────
pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    Json(req):    Json<ChatRequest>,
) -> Result<Json<ChatResponse>, (StatusCode, String)> {

    // Extract the latest user message
    let user_msg = req.messages.iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.trim().to_string())
        .unwrap_or_default();

    if user_msg.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Empty user message".into()));
    }

    // ── Built-in commands (no LLM needed) ────────────────────────────────────
    if let Some(reply) = handle_builtin(&user_msg, &state) {
        return Ok(Json(make_response(&req.model, &reply, 0, reply.len() as u32 / 4)));
    }

    // ── Slash-command routing ─────────────────────────────────────────────────
    let (final_reply, prompt_len) = if let Some((cmd, args)) = parse_slash_command(&user_msg) {
        // Route to plugin
        let plugin_result = {
            let mut pm = state.plugin_manager.lock()
                .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Plugin manager lock poisoned".into()))?;
            pm.call(cmd, args)
        };

        match plugin_result {
            Ok(response) => {
                let raw_json = serde_json::to_string(&response).unwrap_or_default();
                let summary  = summarise_plugin_result(&raw_json);

                if response.success {
                    // Build prompt with plugin context and run LLM for natural language
                    let history = state.history.lock().unwrap();
                    let prompt  = build_prompt(&history, &user_msg, Some(&summary));
                    let prompt_len = prompt.len() as u32 / 4;
                    drop(history);

                    let raw_reply = state.llm_client.complete(&prompt, req.max_tokens).await
                        .unwrap_or_else(|_| summary.clone()); // fallback to raw summary on LLM error

                    let reply = clean_response(&raw_reply);
                    (reply, prompt_len)
                } else {
                    // Plugin failed — give a clear error without LLM
                    let err_msg = response.error.unwrap_or_else(|| "Unknown plugin error".into());
                    (format!("⚠ Command failed: {}", err_msg), 0)
                }
            }
            Err(PluginError::NotFound(cmd)) => {
                (format!("Unknown command: /{}. Type /help to see available commands.", cmd), 0)
            }
            Err(PluginError::Timeout(name)) => {
                (format!("⚠ Plugin {} timed out. The Raspberry Pi may be under load.", name), 0)
            }
            Err(e) => {
                (format!("⚠ {}", e), 0)
            }
        }
    } else {
        // ── Regular chat → LLM ───────────────────────────────────────────────
        let history    = state.history.lock().unwrap();
        let prompt     = build_prompt(&history, &user_msg, None);
        let prompt_len = prompt.len() as u32 / 4;
        drop(history);

        let raw_reply = state.llm_client.complete(&prompt, req.max_tokens).await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("LLM error: {}", e)))?;

        let reply = clean_response(&raw_reply);
        (reply, prompt_len)
    };

    // ── Update conversation history ───────────────────────────────────────────
    {
        let mut history = state.history.lock().unwrap();
        history.push(Role::User, user_msg);
        history.push(Role::Assistant, final_reply.clone());
    }

    let completion_tokens = final_reply.len() as u32 / 4;
    Ok(Json(make_response(&req.model, &final_reply, prompt_len, completion_tokens)))
}

// ─── Built-in commands ────────────────────────────────────────────────────────
fn handle_builtin(msg: &str, state: &Arc<AppState>) -> Option<String> {
    let lower = msg.trim().to_lowercase();

    match lower.as_str() {
        "/help" | "help" => {
            let pm       = state.plugin_manager.lock().ok()?;
            let commands = pm.list_commands();
            Some(format_help(&commands))
        }
        "/reset" | "/clear" => {
            let mut history = state.history.lock().ok()?;
            history.clear();
            Some("Conversation history cleared.".into())
        }
        "/status" => {
            let pm = state.plugin_manager.lock().ok()?;
            let n  = pm.list_commands().len();
            Some(format!("Fabio-Claw is running. {} commands available. Type /help for the full list.", n))
        }
        _ => None,
    }
}

// ─── Response builder ─────────────────────────────────────────────────────────
fn make_response(
    model:             &str,
    content:           &str,
    prompt_tokens:     u32,
    completion_tokens: u32,
) -> ChatResponse {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    ChatResponse {
        id:      format!("chatcmpl-{}", ts),
        object:  "chat.completion".into(),
        created: ts,
        model:   model.to_string(),
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role:    "assistant".into(),
                content: content.to_string(),
            },
            finish_reason: "stop".into(),
        }],
        usage: TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        },
    }
}
