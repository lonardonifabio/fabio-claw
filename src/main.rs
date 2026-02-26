mod agent;
mod api;
mod errors;
mod llm;
mod memory;
mod plugins;
mod scheduler;
mod security;
mod telegram;

use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, fmt};

use crate::api::AppState;
use crate::llm::LlmActor;
use crate::memory::MemoryStore;
use crate::security::DeviceIdentity;

/// Configuration loaded from environment variables with sensible defaults.
struct Config {
    host: String,
    port: u16,
    model_path: String,
    db_path: String,
    key_path: String,
    plugin_dir: String,
}

impl Config {
    fn from_env() -> Self {
        Self {
            host: std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: std::env::var("PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8080),
            model_path: std::env::var("MODEL_PATH")
                .unwrap_or_else(|_| "/opt/fabio-claw/models/model.gguf".into()),
            db_path: std::env::var("DB_PATH")
                .unwrap_or_else(|_| "/var/lib/fabio-claw/memory.db".into()),
            key_path: std::env::var("KEY_PATH")
                .unwrap_or_else(|_| "/var/lib/fabio-claw/device.key".into()),
            plugin_dir: std::env::var("PLUGIN_DIR")
                .unwrap_or_else(|_| "/opt/fabio-claw/plugins".into()),
        }
    }
}

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,fabio_claw=debug"));

    fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();

    info!("🦀 Fabio-Claw v{} starting", env!("CARGO_PKG_VERSION"));

    let config = Config::from_env();

    // Plugin registry
    let plugins = Arc::new(plugins::PluginRegistry::load(&config.plugin_dir));

    // Device identity
    let identity: Arc<DeviceIdentity> = match DeviceIdentity::load_or_generate(&config.key_path) {
        Ok(id) => {
            info!(device_id = %id.public_key_hex(), "Device identity loaded");
            Arc::new(id)
        }
        Err(e) => {
            error!(error = %e, "Failed to initialize device identity");
            std::process::exit(1);
        }
    };

    // Memory store
    let memory: Arc<MemoryStore> = match MemoryStore::open(&config.db_path) {
        Ok(m) => Arc::new(m),
        Err(e) => {
            error!(error = %e, db_path = %config.db_path, "Failed to open memory store");
            std::process::exit(1);
        }
    };

    // Seed default scheduled tasks (won't overwrite existing ones)
    scheduler::seed_default_tasks(&memory).await;

    // LLM actor
    let llm: Arc<LlmActor> = match LlmActor::spawn(config.model_path.clone()) {
        Ok(actor) => Arc::new(actor),
        Err(e) => {
            error!(error = %e, "Failed to initialize LLM actor");
            std::process::exit(1);
        }
    };

    let state = AppState {
        llm:     llm.clone(),
        memory:  memory.clone(),
        device:  identity.clone(),
        plugins: plugins.clone(),
    };

    // Start scheduler as background task
    let sched = Arc::new(scheduler::Scheduler::new(
        memory.clone(),
        plugins.clone(),
        identity.clone(),
        &config.plugin_dir,
    ));
    sched.start();

    // Start Telegram bot if TELEGRAM_TOKEN is set
    let api_base = format!("http://127.0.0.1:{}", config.port);
    if let Some(bot) = telegram::from_env(&api_base) {
        let bot_clone = bot.clone();
        tokio::spawn(async move { bot_clone.run().await });
    }

    // HTTP server
    let app = crate::api::router(state).layer(
        tower_http::cors::CorsLayer::permissive(),
    );

    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .expect("Invalid bind address");

    info!(addr = %addr, "HTTP server listening");
    info!("OpenAI endpoint:  http://{}/v1/chat/completions", addr);
    info!("Health check:     http://{}/health", addr);
    info!("Tool definitions: http://{}/v1/tools", addr);
    info!("Scheduled tasks:  http://{}/v1/tasks", addr);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind TCP listener");

    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("Server error");

    info!("Fabio-Claw shutdown complete");
}

async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let sigterm = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c  => info!("Received Ctrl-C"),
        _ = sigterm => info!("Received SIGTERM"),
    }

    info!("Initiating graceful shutdown...");
}
