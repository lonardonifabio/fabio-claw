// src/llm_client.rs
//
// Abstraction over the local LLM inference server (llama.cpp / ollama).
// Handles:
//   1. Completion via /completion endpoint (llama.cpp server)
//   2. Fallback to /api/generate (Ollama)
//   3. Configurable temperature and repeat penalty (critical for TinyLlama quality)
//   4. Async with tokio
//   5. Timeout to prevent hanging the Pi

use std::time::Duration;
use serde::{Deserialize, Serialize};

// ─── Config ───────────────────────────────────────────────────────────────────
// Tuned specifically for TinyLlama 1.1B Q4:
//   - temperature 0.7 → balanced creativity vs accuracy
//   - repeat_penalty 1.15 → reduces TinyLlama's tendency to repeat itself
//   - top_p 0.9 → nucleus sampling for better coherence
//   - top_k 40 → limit vocabulary breadth (helps small models)
//   - stop sequences → prevent prompt echo and ChatML token leakage
const LLM_HOST:          &str = "http://localhost";
const LLM_PORT_LLAMACPP: u16  = 8080;
const LLM_PORT_OLLAMA:   u16  = 11434;
const REQUEST_TIMEOUT_MS: u64 = 30_000;  // 30s — Pi can be slow

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub base_url:       String,
    pub backend:        LlmBackend,
    pub model_name:     String,
    pub temperature:    f32,
    pub repeat_penalty: f32,
    pub top_p:          f32,
    pub top_k:          u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LlmBackend {
    LlamaCpp,  // llama.cpp server (/completion endpoint)
    Ollama,    // Ollama (/api/generate endpoint)
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            base_url:       format!("{}:{}", LLM_HOST, LLM_PORT_LLAMACPP),
            backend:        LlmBackend::LlamaCpp,
            model_name:     "tinyllama".into(),
            // TinyLlama 1.1B Q4 — quality-tuned parameters:
            temperature:    0.7,
            repeat_penalty: 1.15,
            top_p:          0.9,
            top_k:          40,
        }
    }
}

impl LlmConfig {
    /// Auto-detect backend by trying both ports.
    pub async fn auto_detect() -> Self {
        let client = reqwest::Client::new();

        // Try llama.cpp first
        if client.get(&format!("{}:{}/health", LLM_HOST, LLM_PORT_LLAMACPP))
            .timeout(Duration::from_secs(2))
            .send().await.is_ok()
        {
            println!("[LlmClient] Detected llama.cpp server on port {}", LLM_PORT_LLAMACPP);
            return Self {
                base_url: format!("{}:{}", LLM_HOST, LLM_PORT_LLAMACPP),
                backend:  LlmBackend::LlamaCpp,
                ..Default::default()
            };
        }

        // Try Ollama
        if client.get(&format!("{}:{}/api/tags", LLM_HOST, LLM_PORT_OLLAMA))
            .timeout(Duration::from_secs(2))
            .send().await.is_ok()
        {
            println!("[LlmClient] Detected Ollama on port {}", LLM_PORT_OLLAMA);
            return Self {
                base_url: format!("{}:{}", LLM_HOST, LLM_PORT_OLLAMA),
                backend:  LlmBackend::Ollama,
                ..Default::default()
            };
        }

        println!("[LlmClient] Warning: no LLM server detected. Using default config.");
        Self::default()
    }
}

// ─── LLM client ───────────────────────────────────────────────────────────────
pub struct LlmClient {
    config: LlmConfig,
    http:   reqwest::Client,
}

impl LlmClient {
    pub fn new(config: LlmConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(REQUEST_TIMEOUT_MS))
            .build()
            .expect("Failed to build HTTP client");
        Self { config, http }
    }

    /// Send a fully-formatted prompt and return the generated text.
    pub async fn complete(&self, prompt: &str, max_tokens: u32) -> Result<String, LlmError> {
        match self.config.backend {
            LlmBackend::LlamaCpp => self.complete_llamacpp(prompt, max_tokens).await,
            LlmBackend::Ollama   => self.complete_ollama(prompt, max_tokens).await,
        }
    }

    // ── llama.cpp /completion ─────────────────────────────────────────────────
    async fn complete_llamacpp(&self, prompt: &str, max_tokens: u32) -> Result<String, LlmError> {
        #[derive(Serialize)]
        struct LlamaCppRequest<'a> {
            prompt:           &'a str,
            n_predict:        u32,
            temperature:      f32,
            repeat_penalty:   f32,
            top_p:            f32,
            top_k:            u32,
            stop:             Vec<&'a str>,
            stream:           bool,
            // TinyLlama-specific: stop at special tokens
            #[serde(rename = "n_keep")]
            n_keep:           i32,
        }

        #[derive(Deserialize)]
        struct LlamaCppResponse {
            content: String,
        }

        let body = LlamaCppRequest {
            prompt,
            n_predict:        max_tokens.min(512),
            temperature:      self.config.temperature,
            repeat_penalty:   self.config.repeat_penalty,
            top_p:            self.config.top_p,
            top_k:            self.config.top_k,
            // Stop sequences prevent prompt echo and token leakage
            stop:             vec!["<|user|>", "<|system|>", "</s>", "\n\nUser:", "\nUser:"],
            stream:           false,
            n_keep:           -1,  // keep all prompt tokens in KV cache
        };

        let url = format!("{}/completion", self.config.base_url);
        let resp = self.http.post(&url)
            .json(&body)
            .send().await
            .map_err(|e| LlmError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(LlmError::Server(resp.status().as_u16(), resp.text().await.unwrap_or_default()));
        }

        let data: LlamaCppResponse = resp.json().await
            .map_err(|e| LlmError::Parse(e.to_string()))?;

        Ok(data.content)
    }

    // ── Ollama /api/generate ──────────────────────────────────────────────────
    async fn complete_ollama(&self, prompt: &str, max_tokens: u32) -> Result<String, LlmError> {
        #[derive(Serialize)]
        struct OllamaRequest<'a> {
            model:   &'a str,
            prompt:  &'a str,
            stream:  bool,
            options: OllamaOptions,
        }

        #[derive(Serialize)]
        struct OllamaOptions {
            temperature:    f32,
            repeat_penalty: f32,
            top_p:          f32,
            top_k:          u32,
            num_predict:    u32,
            stop:           Vec<String>,
        }

        #[derive(Deserialize)]
        struct OllamaResponse {
            response: String,
        }

        let body = OllamaRequest {
            model:  &self.config.model_name,
            prompt,
            stream: false,
            options: OllamaOptions {
                temperature:    self.config.temperature,
                repeat_penalty: self.config.repeat_penalty,
                top_p:          self.config.top_p,
                top_k:          self.config.top_k,
                num_predict:    max_tokens.min(512),
                stop:           vec![
                    "<|user|>".into(), "<|system|>".into(),
                    "</s>".into(), "\n\nUser:".into(),
                ],
            },
        };

        let url = format!("{}/api/generate", self.config.base_url);
        let resp = self.http.post(&url)
            .json(&body)
            .send().await
            .map_err(|e| LlmError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(LlmError::Server(resp.status().as_u16(), resp.text().await.unwrap_or_default()));
        }

        let data: OllamaResponse = resp.json().await
            .map_err(|e| LlmError::Parse(e.to_string()))?;

        Ok(data.response)
    }
}

// ─── Error type ───────────────────────────────────────────────────────────────
#[derive(Debug)]
pub enum LlmError {
    Network(String),
    Server(u16, String),
    Parse(String),
    Timeout,
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::Network(e)    => write!(f, "LLM network error: {}", e),
            LlmError::Server(c, e)  => write!(f, "LLM server error {}: {}", c, e),
            LlmError::Parse(e)      => write!(f, "LLM response parse error: {}", e),
            LlmError::Timeout       => write!(f, "LLM request timed out"),
        }
    }
}
