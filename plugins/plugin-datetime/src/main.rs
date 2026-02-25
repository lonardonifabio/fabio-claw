use serde::{Deserialize, Serialize};
use serde_json::Value;
use chrono::Local;
use std::io::{self, Read};

#[derive(Debug, Deserialize)]
struct PluginRequest {
    action: String,
    payload: Value,
}

#[derive(Debug, Serialize)]
struct PluginResponse {
    success: bool,
    result: Value,
    error: Option<String>,
}

fn main() {
    // Read full STDIN
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap_or(0);

    let response = match serde_json::from_str::<PluginRequest>(&input) {
        Ok(req) => handle(req),
        Err(e) => PluginResponse {
            success: false,
            result: Value::Null,
            error: Some(format!("Invalid request JSON: {}", e)),
        },
    };

    println!("{}", serde_json::to_string(&response).unwrap());
}

fn handle(req: PluginRequest) -> PluginResponse {
    match req.action.as_str() {
        "now" | "datetime" | "time" | "date" => {
            let now = Local::now();
            PluginResponse {
                success: true,
                result: serde_json::json!({
                    "datetime":    now.format("%Y-%m-%d %H:%M:%S").to_string(),
                    "date":        now.format("%Y-%m-%d").to_string(),
                    "time":        now.format("%H:%M:%S").to_string(),
                    "day_of_week": now.format("%A").to_string(),
                    "timezone":    now.format("%Z").to_string(),
                    "unix":        now.timestamp(),
                }),
                error: None,
            }
        }
        _ => PluginResponse {
            success: false,
            result: Value::Null,
            error: Some(format!("Unknown action '{}'. Supported: now, datetime, time, date", req.action)),
        },
    }
}
