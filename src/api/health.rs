use axum::{extract::State, Json};
use serde::Serialize;
use chrono::Utc;

use crate::api::AppState;

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub timestamp: String,
    pub version: String,
    pub device_id: String,
}

#[derive(Serialize)]
pub struct ReadinessResponse {
    pub ready: bool,
    pub llm_loaded: bool,
    pub memory_ok: bool,
}

pub async fn health_check(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        timestamp: Utc::now().to_rfc3339(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        device_id: state.device.public_key_hex(),
    })
}

pub async fn readiness_check(State(state): State<AppState>) -> Json<ReadinessResponse> {
    let llm_loaded = state.llm.is_ready();
    let memory_ok: bool = state.memory.ping().await.is_ok();

    Json(ReadinessResponse {
        ready: llm_loaded && memory_ok,
        llm_loaded,
        memory_ok,
    })
}
