pub mod chat;
pub mod health;
pub mod models;

use axum::{Router, routing::{get, post}};
use std::sync::Arc;
use crate::llm::LlmActor;
use crate::memory::MemoryStore;
use crate::security::DeviceIdentity;
use crate::plugins::PluginRegistry;

#[derive(Clone)]
pub struct AppState {
    pub llm:     Arc<LlmActor>,
    pub memory:  Arc<MemoryStore>,
    pub device:  Arc<DeviceIdentity>,
    pub plugins: Arc<PluginRegistry>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(chat::chat_completions))
        .route("/v1/models",           get(models::list_models))
        .route("/health",              get(health::health_check))
        .route("/health/ready",        get(health::readiness_check))
        .with_state(state)
}
