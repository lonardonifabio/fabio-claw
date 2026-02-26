pub mod chat;
pub mod health;
pub mod models;
pub mod scheduler;

use axum::{Router, routing::{get, post, delete}};
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
        .route("/v1/chat/completions",    post(chat::chat_completions))
        .route("/v1/models",              get(models::list_models))
        .route("/health",                 get(health::health_check))
        .route("/health/ready",           get(health::readiness_check))
        // Scheduler management
        .route("/v1/tasks",               get(scheduler::list_tasks))
        .route("/v1/tasks",               post(scheduler::create_task))
        .route("/v1/tasks/:name",         delete(scheduler::delete_task))
        // Plugin tool definitions (for OpenAI tool-calling clients)
        .route("/v1/tools",               get(tools_list))
        .with_state(state)
}

async fn tools_list(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "tools": state.plugins.as_tools()
    }))
}
