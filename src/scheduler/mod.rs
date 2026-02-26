//! Persistent cron-style task scheduler backed by SQLite.
//!
//! Tasks are stored in the `scheduled_tasks` table and survive restarts.
//! Each task specifies a cron expression, a plugin command, and optional args.
//!
//! The scheduler runs as a background Tokio task that wakes up every 30 seconds,
//! checks for due tasks, fires them, and updates `last_run` / `next_run`.

use std::str::FromStr;
use std::sync::Arc;
use chrono::{DateTime, TimeZone, Utc};
use cron::Schedule;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};

use crate::errors::AppError;
use crate::memory::{MemoryStore, ScheduledTask};
use crate::plugins::{PluginRegistry, PluginRequest, PluginRunner};
use crate::security::DeviceIdentity;

const POLL_INTERVAL_SECS: u64 = 30;

pub struct Scheduler {
    memory: Arc<MemoryStore>,
    plugins: Arc<PluginRegistry>,
    device: Arc<DeviceIdentity>,
    plugin_runner: PluginRunner,
}

impl Scheduler {
    pub fn new(
        memory: Arc<MemoryStore>,
        plugins: Arc<PluginRegistry>,
        device: Arc<DeviceIdentity>,
        plugin_dir: &str,
    ) -> Self {
        Self {
            memory,
            plugins,
            device,
            plugin_runner: PluginRunner::new(plugin_dir),
        }
    }

    /// Spawn the scheduler as a background task. Returns immediately.
    pub fn start(self: Arc<Self>) {
        tokio::spawn(async move {
            info!("Scheduler started (poll every {}s)", POLL_INTERVAL_SECS);
            let mut ticker = interval(Duration::from_secs(POLL_INTERVAL_SECS));
            loop {
                ticker.tick().await;
                if let Err(e) = self.tick().await {
                    error!(error = %e, "Scheduler tick error");
                }
            }
        });
    }

    async fn tick(&self) -> Result<(), AppError> {
        let now = Utc::now();
        let tasks = self.memory.list_tasks().await?;

        for task in tasks {
            if !task.enabled { continue; }

            let due = match &task.next_run {
                Some(next) => now >= *next,
                None => {
                    // First run: schedule immediately
                    true
                }
            };

            if due {
                info!(task = %task.name, command = %task.command, "Running scheduled task");
                self.run_task(&task).await;

                // Calculate next run time
                let next = next_run_time(&task.cron_expr, now);
                self.memory
                    .update_task_run_times(&task.name, now, next)
                    .await
                    .unwrap_or_else(|e| warn!(error = %e, "Failed to update task times"));
            }
        }
        Ok(())
    }

    async fn run_task(&self, task: &ScheduledTask) {
        // Resolve the command as a plugin
        let manifest = match self.plugins
            .resolve(&task.command)
            .or_else(|| self.plugins.resolve_by_name(&task.command))
        {
            Some(m) => m.clone(),
            None => {
                warn!(task = %task.name, command = %task.command, "Unknown command in scheduled task");
                return;
            }
        };

        let request = PluginRequest {
            action: manifest.default_action.clone(),
            payload: serde_json::json!({ "args": task.args }),
        };

        match self.plugin_runner.run(&manifest, &request, &self.device).await {
            Ok(resp) if resp.success => {
                info!(task = %task.name, result = %resp.result, "Scheduled task completed");
            }
            Ok(resp) => {
                warn!(task = %task.name, error = ?resp.error, "Scheduled task plugin error");
            }
            Err(e) => {
                error!(task = %task.name, error = %e, "Scheduled task failed");
            }
        }
    }
}

/// Parse a cron expression and compute the next fire time after `after`.
pub fn next_run_time(cron_expr: &str, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let schedule = Schedule::from_str(cron_expr).ok()?;
    schedule.after(&after).next()
        .map(|t| Utc.from_utc_datetime(&t.naive_utc()))
}

/// Register default system tasks (called once at startup if they don't exist yet).
pub async fn seed_default_tasks(memory: &MemoryStore) {
    let defaults: &[(&str, &str, &str, &str)] = &[
        // (name, cron_expr, command, args)
        ("hourly-datetime", "0 * * * * *", "datetime", ""),
    ];

    for (name, cron_expr, command, args) in defaults {
        let next = next_run_time(cron_expr, Utc::now());
        let task = ScheduledTask {
            id: None,
            name: name.to_string(),
            cron_expr: cron_expr.to_string(),
            command: command.to_string(),
            args: args.to_string(),
            enabled: false, // disabled by default, user must enable
            last_run: None,
            next_run: next,
            created_at: Utc::now(),
        };
        if let Err(e) = memory.upsert_task(&task).await {
            warn!(task = name, error = %e, "Failed to seed default task");
        }
    }
}
