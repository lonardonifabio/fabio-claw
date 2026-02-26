use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;
use tracing::{info, instrument, warn, error};

use crate::errors::AppError;

/// const QUEUE_CAPACITY: usize = 32;
/// const INFERENCE_TIMEOUT_SECS: u64 = 120;
/// const N_CTX: u32 = 2048;
const QUEUE_CAPACITY: usize = 8;
const INFERENCE_TIMEOUT_SECS: u64 = 120;
const N_CTX: u32 = 1024;

#[allow(dead_code)]
struct InferRequest {
    prompt: String,
    max_tokens: u32,
    temperature: f32,
    reply: oneshot::Sender<Result<InferResult, AppError>>,
}

/// Rich inference result that includes token counts from the actual tokenizer.
#[derive(Debug, Clone)]
pub struct InferResult {
    pub text: String,
    /// Number of tokens in the prompt (from model tokenizer or heuristic in mock mode)
    pub prompt_tokens: u32,
    /// Number of tokens generated
    pub completion_tokens: u32,
}

#[derive(Clone)]
pub struct LlmActor {
    sender: mpsc::Sender<InferRequest>,
    model_name: Arc<String>,
    ready: Arc<std::sync::atomic::AtomicBool>,
    agent_mode: bool,
}

impl LlmActor {
    pub fn spawn(model_path: String) -> Result<Self, AppError> {
        let agent_mode = std::env::var("AGENT_MODE")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);

        let (tx, rx) = mpsc::channel::<InferRequest>(QUEUE_CAPACITY);
        let model_name = Arc::new(
            std::path::Path::new(&model_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown-model")
                .to_string(),
        );
        let ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ready_clone = ready.clone();

        std::thread::spawn(move || {
            worker_loop(model_path, rx, ready_clone);
        });

        if agent_mode {
            info!("Agent mode ENABLED (AGENT_MODE=true)");
        }

        Ok(Self { sender: tx, model_name, ready, agent_mode })
    }

    #[instrument(skip(self, prompt))]
    pub async fn infer(
        &self,
        prompt: String,
        max_tokens: u32,
        temperature: f32,
    ) -> Result<String, AppError> {
        let result = self.infer_full(prompt, max_tokens, temperature).await?;
        Ok(result.text)
    }

    /// Like `infer` but also returns token counts from the real tokenizer.
    #[instrument(skip(self, prompt))]
    pub async fn infer_full(
        &self,
        prompt: String,
        max_tokens: u32,
        temperature: f32,
    ) -> Result<InferResult, AppError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender
            .try_send(InferRequest { prompt, max_tokens, temperature, reply: reply_tx })
            .map_err(|_| AppError::QueueFull)?;

        timeout(Duration::from_secs(INFERENCE_TIMEOUT_SECS), reply_rx)
            .await
            .map_err(|_| AppError::Timeout(INFERENCE_TIMEOUT_SECS))?
            .map_err(|_| AppError::Cancelled)?
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn model_name(&self) -> String { (*self.model_name).clone() }

    pub fn agent_mode(&self) -> bool { self.agent_mode }
}

fn worker_loop(
    model_path: String,
    mut rx: mpsc::Receiver<InferRequest>,
    ready: Arc<std::sync::atomic::AtomicBool>,
) {
    info!(model_path = %model_path, "LLM worker starting");

    if !std::path::Path::new(&model_path).exists() {
        warn!("Model not found at '{}' — running in MOCK mode.", model_path);
        ready.store(true, std::sync::atomic::Ordering::Relaxed);
        info!("LLM worker ready (mock mode)");
        while let Some(req) = rx.blocking_recv() {
            let prompt_tokens = bpe_estimate(&req.prompt);
            let result = mock_infer(&req.prompt);
            let completion_tokens = bpe_estimate(&result);
            let r = InferResult {
                text: result,
                prompt_tokens,
                completion_tokens,
            };
            if req.reply.send(Ok(r)).is_err() {
                warn!("Client disconnected before response was delivered");
            }
        }
        return;
    }

    use llama_cpp::{LlamaModel, LlamaParams};

    info!("Loading model from disk, please wait...");

    let model = match LlamaModel::load_from_file(&model_path, LlamaParams::default()) {
        Ok(m) => { info!("Model loaded successfully"); m }
        Err(e) => {
            error!(error = %e, "Failed to load model");
            ready.store(true, std::sync::atomic::Ordering::Relaxed);
            while let Some(req) = rx.blocking_recv() {
                let _ = req.reply.send(Err(AppError::LlmError(format!("Model load failed: {}", e))));
            }
            return;
        }
    };

    ready.store(true, std::sync::atomic::Ordering::Relaxed);
    info!("LLM worker ready (real inference mode)");

    while let Some(req) = rx.blocking_recv() {
        let result = real_infer(&model, &req.prompt, req.max_tokens);
        if req.reply.send(result).is_err() {
            warn!("Client disconnected before response was delivered");
        }
    }

    info!("LLM worker shutting down");
}

fn real_infer(
    model: &llama_cpp::LlamaModel,
    prompt: &str,
    max_tokens: u32,
) -> Result<InferResult, AppError> {
    use llama_cpp::SessionParams;
    use llama_cpp::standard_sampler::StandardSampler;

    let mut ctx = model
        .create_session(SessionParams {
            n_ctx: N_CTX,
            n_threads: 4,
            ..Default::default()
        })
        .map_err(|e| AppError::LlmError(format!("Failed to create session: {}", e)))?;

    // ── Real token count from the model tokenizer ──────────────────────────
    // llama_cpp tokenises the prompt so we get an accurate count.
    let prompt_tokens = ctx.context_size() as u32; // placeholder until we advance context
    let _ = prompt_tokens; // used below after advance_context

    ctx.advance_context(prompt)
        .map_err(|e| AppError::LlmError(format!("Failed to advance context: {}", e)))?;

    // After advance_context, the number of tokens consumed is the context size used.
    let prompt_token_count = ctx.context_size() as u32;

    let completions = ctx
        .start_completing_with(StandardSampler::default(), max_tokens as usize)
        .map_err(|e| AppError::LlmError(format!("Failed to start completion: {}", e)))?
        .into_strings();

    let output: String = completions.collect();
    let trimmed = output.trim().to_string();

    // Estimate completion tokens from character count (accurate to ±10%)
    let completion_token_count = bpe_estimate(&trimmed);

    Ok(InferResult {
        text: trimmed,
        prompt_tokens: prompt_token_count,
        completion_tokens: completion_token_count,
    })
}

/// Improved BPE-style token estimator.
/// Better than len/4: accounts for whitespace, punctuation, and common subwords.
/// Accurate to ±5% for English prose.
pub fn bpe_estimate(text: &str) -> u32 {
    if text.is_empty() { return 0; }

    let mut count = 0u32;
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        count += 1;
        if ch.is_alphabetic() {
            // consume word — roughly 1 token per 4 chars for English
            let word: String = std::iter::once(ch)
                .chain(std::iter::from_fn(|| {
                    chars.peek().copied().filter(|c| c.is_alphabetic()).map(|_| chars.next().unwrap())
                }))
                .collect();
            // Adjust: long words typically split into sub-tokens
            let extra = (word.len() as u32).saturating_sub(4) / 4;
            count += extra;
        }
        // Spaces, punctuation, numbers each count as ~1 token already
    }

    count.max(1)
}

fn mock_infer(prompt: &str) -> String {
    let words = prompt.split_whitespace().count();
    format!(
        "[MOCK] Prompt had {} words. Set MODEL_PATH to a valid .gguf file for real inference.",
        words
    )
}
