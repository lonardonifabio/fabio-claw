use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;
use tracing::{info, instrument, warn, error};

use crate::errors::AppError;

const QUEUE_CAPACITY: usize = 32;
const INFERENCE_TIMEOUT_SECS: u64 = 120;
const N_CTX: u32 = 2048;

#[allow(dead_code)]
struct InferRequest {
    prompt: String,
    max_tokens: u32,
    temperature: f32,
    reply: oneshot::Sender<Result<String, AppError>>,
}

#[derive(Clone)]
pub struct LlmActor {
    sender: mpsc::Sender<InferRequest>,
    model_name: Arc<String>,
    ready: Arc<std::sync::atomic::AtomicBool>,
}

impl LlmActor {
    pub fn spawn(model_path: String) -> Result<Self, AppError> {
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

        Ok(Self { sender: tx, model_name, ready })
    }

    #[instrument(skip(self, prompt))]
    pub async fn infer(
        &self,
        prompt: String,
        max_tokens: u32,
        temperature: f32,
    ) -> Result<String, AppError> {
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

    pub fn model_name(&self) -> String {
        (*self.model_name).clone()
    }
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
            if req.reply.send(mock_infer(&req.prompt)).is_err() {
                warn!("Client disconnected before response was delivered");
            }
        }
        return;
    }

    // --- Real llama.cpp inference ---
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
) -> Result<String, AppError> {
    use llama_cpp::SessionParams;
    use llama_cpp::standard_sampler::StandardSampler;

    let mut ctx = model
        .create_session(SessionParams { n_ctx: N_CTX, n_threads: 4, ..Default::default() })
        .map_err(|e| AppError::LlmError(format!("Failed to create session: {}", e)))?;

    ctx.advance_context(prompt)
        .map_err(|e| AppError::LlmError(format!("Failed to advance context: {}", e)))?;

    let completions = ctx
        .start_completing_with(StandardSampler::default(), max_tokens as usize)
        .map_err(|e| AppError::LlmError(format!("Failed to start completion: {}", e)))?
        .into_strings();

    // into_strings() yields String directly (not Result<String>)
    let output: String = completions.collect();

    Ok(output.trim().to_string())
}

fn mock_infer(prompt: &str) -> Result<String, AppError> {
    let words = prompt.split_whitespace().count();
    Ok(format!(
        "[MOCK] Prompt had {} words. Set MODEL_PATH to a valid .gguf file for real inference.",
        words
    ))
}
