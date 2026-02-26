use axum::{extract::{Path, State}, Json};
use serde::{Deserialize, Serialize};
use chrono::Utc;
use tracing::info;

use crate::api::AppState;
use crate::errors::AppError;
use crate::memory::ScheduledTask;
use crate::scheduler::next_run_time;

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub name: String,
    /// Standard 6-field cron expression (seconds precision): "0 * * * * *"
    pub cron_expr: String,
    /// Plugin slash-command or name
    pub command: String,
    #[serde(default)]
    pub args: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool { true }

#[derive(Debug, Serialize)]
pub struct TaskResponse {
    pub name: String,
    pub cron_expr: String,
    pub command: String,
    pub args: String,
    pub enabled: bool,
    pub last_run: Option<String>,
    pub next_run: Option<String>,
    pub created_at: String,
}

impl From<ScheduledTask> for TaskResponse {
    fn from(t: ScheduledTask) -> Self {
        Self {
            name: t.name,
            cron_expr: t.cron_expr,
            command: t.command,
            args: t.args,
            enabled: t.enabled,
            last_run: t.last_run.map(|d| d.to_rfc3339()),
            next_run: t.next_run.map(|d| d.to_rfc3339()),
            created_at: t.created_at.to_rfc3339(),
        }
    }
}

pub async fn list_tasks(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let tasks = state.memory.list_tasks().await?;
    let resp: Vec<TaskResponse> = tasks.into_iter().map(Into::into).collect();
    Ok(Json(serde_json::json!({ "tasks": resp, "count": resp.len() })))
}

pub async fn create_task(
    State(state): State<AppState>,
    Json(req): Json<CreateTaskRequest>,
) -> Result<Json<TaskResponse>, AppError> {
    if req.name.is_empty() {
        return Err(AppError::InvalidRequest("Task name cannot be empty".into()));
    }
    if state.plugins.resolve(&req.command).is_none()
        && state.plugins.resolve_by_name(&req.command).is_none()
    {
        return Err(AppError::InvalidRequest(format!(
            "Unknown command '{}'. Type /help to see available commands.",
            req.command
        )));
    }

    let next = next_run_time(&req.cron_expr, Utc::now());
    let task = ScheduledTask {
        id: None,
        name: req.name.clone(),
        cron_expr: req.cron_expr,
        command: req.command,
        args: req.args,
        enabled: req.enabled,
        last_run: None,
        next_run: next,
        created_at: Utc::now(),
    };

    state.memory.upsert_task(&task).await?;
    info!(name = %req.name, "Scheduled task created");

    let tasks = state.memory.list_tasks().await?;
    let saved = tasks.into_iter().find(|t| t.name == req.name)
        .ok_or_else(|| AppError::SchedulerError("Task not found after creation".into()))?;

    Ok(Json(saved.into()))
}

pub async fn delete_task(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    state.memory.delete_task(&name).await?;
    info!(name = %name, "Scheduled task deleted");
    Ok(Json(serde_json::json!({ "deleted": name })))
}
