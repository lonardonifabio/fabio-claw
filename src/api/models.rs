use axum::{extract::State, Json};
use serde::Serialize;
use chrono::Utc;

use crate::api::AppState;

#[derive(Serialize)]
pub struct ModelsResponse {
    pub object: String,
    pub data: Vec<ModelInfo>,
}

#[derive(Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub owned_by: String,
}

pub async fn list_models(State(state): State<AppState>) -> Json<ModelsResponse> {
    let model_id = state.llm.model_name();
    Json(ModelsResponse {
        object: "list".to_string(),
        data: vec![ModelInfo {
            id: model_id,
            object: "model".to_string(),
            created: Utc::now().timestamp(),
            owned_by: "fabio-claw-edge".to_string(),
        }],
    })
}
