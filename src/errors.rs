#![allow(dead_code)]
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("LLM inference error: {0}")]
    LlmError(String),

    #[error("Database error: {0}")]
    DatabaseError(#[from] rusqlite::Error),

    #[error("Plugin error: {0}")]
    PluginError(String),

    #[error("Security error: {0}")]
    SecurityError(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Queue full - server overloaded")]
    QueueFull,

    #[error("Inference timeout after {0}s")]
    Timeout(u64),

    #[error("Request cancelled")]
    Cancelled,

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    SerdeError(#[from] serde_json::Error),
}

impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        use axum::Json;

        let (status, message) = match &self {
            AppError::QueueFull => (StatusCode::TOO_MANY_REQUESTS, self.to_string()),
            AppError::Timeout(_) => (StatusCode::GATEWAY_TIMEOUT, self.to_string()),
            AppError::InvalidRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            AppError::SecurityError(_) => (StatusCode::FORBIDDEN, self.to_string()),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
        };

        let body = serde_json::json!({
            "error": {
                "message": message,
                "type": "edge_runtime_error"
            }
        });

        (status, Json(body)).into_response()
    }
}
