//! Telegram integration — lightweight long-polling bot.
//!
//! Activated when `TELEGRAM_TOKEN` env var is set.
//! The bot forwards every message to the chat API (including /commands)
//! and replies with the result, so users get the same capabilities as the
//! browser UI via Telegram.
//!
//! No teloxide dependency needed — we use plain HTTP to the Bot API
//! so this compiles without the optional `telegram` feature flag.

use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tracing::{info, warn, error, debug};

use crate::errors::AppError;

const POLL_TIMEOUT_SECS: u64 = 30;
const MAX_TEXT_LENGTH: usize = 4096; // Telegram message limit

// ─── Telegram API types (minimal) ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TgResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TgUpdate {
    pub update_id: i64,
    pub message: Option<TgMessage>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TgMessage {
    pub message_id: i64,
    pub chat: TgChat,
    pub text: Option<String>,
    pub from: Option<TgUser>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TgChat {
    pub id: i64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TgUser {
    pub id: i64,
    pub first_name: String,
    pub username: Option<String>,
}

#[derive(Debug, Serialize)]
struct SendMessagePayload<'a> {
    chat_id: i64,
    text: &'a str,
    parse_mode: &'a str,
}

// ─── Bot client ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct TelegramBot {
    token: String,
    /// Optional allowlist of chat IDs. If empty, all chats are accepted.
    allowed_chats: Vec<i64>,
    /// Base URL of the local fabio-claw API
    api_base: String,
    http: reqwest::Client,
}

impl TelegramBot {
    pub fn new(token: String, api_base: String, allowed_chats: Vec<i64>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("Failed to create HTTP client");
        Self { token, allowed_chats, api_base, http }
    }

    fn base(&self) -> String {
        format!("https://api.telegram.org/bot{}", self.token)
    }

    /// Start the polling loop — runs indefinitely, never returns.
    pub async fn run(self: Arc<Self>) {
        info!("Telegram bot started (long-polling)");
        let mut offset: i64 = 0;

        loop {
            match self.get_updates(offset).await {
                Ok(updates) => {
                    for update in updates {
                        offset = offset.max(update.update_id + 1);
                        let bot = self.clone();
                        tokio::spawn(async move {
                            bot.handle_update(update).await;
                        });
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Telegram getUpdates error — retrying in 5s");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn get_updates(&self, offset: i64) -> Result<Vec<TgUpdate>, AppError> {
        let url = format!("{}/getUpdates?offset={}&timeout={}", self.base(), offset, POLL_TIMEOUT_SECS);
        let resp: TgResponse<Vec<TgUpdate>> = self.http.get(&url).send().await
            .map_err(|e| AppError::TelegramError(e.to_string()))?
            .json().await
            .map_err(|e| AppError::TelegramError(e.to_string()))?;

        if !resp.ok {
            return Err(AppError::TelegramError(
                resp.description.unwrap_or_else(|| "Unknown Telegram error".into())
            ));
        }
        Ok(resp.result.unwrap_or_default())
    }

    async fn handle_update(&self, update: TgUpdate) {
        let msg = match update.message {
            Some(m) => m,
            None => return,
        };

        let chat_id = msg.chat.id;

        // Allowlist check
        if !self.allowed_chats.is_empty() && !self.allowed_chats.contains(&chat_id) {
            warn!(chat_id, "Ignoring message from non-allowlisted chat");
            return;
        }

        let text = match msg.text {
            Some(t) => t,
            None => return, // ignore non-text messages
        };

        let user = msg.from.as_ref()
            .map(|u| u.username.as_deref().unwrap_or(&u.first_name))
            .unwrap_or("unknown");

        info!(chat_id, user, text = %text, "Telegram message received");

        let reply = self.forward_to_api(&text).await;
        self.send_message(chat_id, &reply).await;
    }

    /// Forward the user text to the local fabio-claw chat API and return the response.
    async fn forward_to_api(&self, text: &str) -> String {
        let url = format!("{}/v1/chat/completions", self.api_base);
        let body = serde_json::json!({
            "model": "local",
            "messages": [
                {"role": "user", "content": text}
            ],
            "max_tokens": 512
        });

        match self.http.post(&url).json(&body).send().await {
            Ok(resp) => {
                match resp.json::<serde_json::Value>().await {
                    Ok(json) => {
                        json["choices"][0]["message"]["content"]
                            .as_str()
                            .unwrap_or("(empty response)")
                            .to_string()
                    }
                    Err(e) => {
                        error!(error = %e, "Failed to parse API response");
                        "⚠️ Internal error — could not parse response.".into()
                    }
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to reach fabio-claw API");
                "⚠️ Could not reach the AI backend. Please try again.".into()
            }
        }
    }

    async fn send_message(&self, chat_id: i64, text: &str) {
        // Split long messages at Telegram's 4096-char limit
        let chunks: Vec<&str> = split_message(text, MAX_TEXT_LENGTH);
        for chunk in chunks {
            let url = format!("{}/sendMessage", self.base());
            let payload = SendMessagePayload { chat_id, text: chunk, parse_mode: "Markdown" };
            match self.http.post(&url).json(&payload).send().await {
                Ok(r) if r.status().is_success() => {
                    debug!(chat_id, "Message sent");
                }
                Ok(r) => {
                    // Fallback: retry without Markdown in case of parse errors
                    warn!(chat_id, status = %r.status(), "sendMessage failed — retrying as plain text");
                    let plain = SendMessagePayload { chat_id, text: chunk, parse_mode: "" };
                    let _ = self.http.post(&url).json(&plain).send().await;
                }
                Err(e) => {
                    error!(chat_id, error = %e, "sendMessage network error");
                }
            }
        }
    }
}

/// Split a string into chunks of at most `max_len` chars, splitting on newlines where possible.
fn split_message(text: &str, max_len: usize) -> Vec<&str> {
    if text.len() <= max_len { return vec![text]; }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = (start + max_len).min(text.len());
        // Try to break at a newline
        let split = if end < text.len() {
            text[start..end].rfind('\n').map(|i| start + i + 1).unwrap_or(end)
        } else {
            end
        };
        chunks.push(&text[start..split]);
        start = split;
    }
    chunks
}

// ─── Startup helper ──────────────────────────────────────────────────────────

/// Returns a configured `TelegramBot` if `TELEGRAM_TOKEN` is set, otherwise `None`.
pub fn from_env(api_base: &str) -> Option<Arc<TelegramBot>> {
    let token = std::env::var("TOKEN").ok()?;

    let allowed_chats: Vec<i64> = std::env::var("TELEGRAM_ALLOWED_CHATS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse::<i64>().ok())
        .collect();

    info!(
        allowed_chats = ?allowed_chats,
        "Telegram integration enabled"
    );

    Some(Arc::new(TelegramBot::new(token, api_base.to_string(), allowed_chats)))
}
