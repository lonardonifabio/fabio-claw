use serde::{Deserialize, Serialize};
use serde_json::Value;
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
        "calculate" | "eval" | "compute" => {
            let expr = match req.payload.get("expression").and_then(|v| v.as_str()) {
                Some(e) => e.to_string(),
                None => {
                    return PluginResponse {
                        success: false,
                        result: Value::Null,
                        error: Some("Missing 'expression' in payload".into()),
                    }
                }
            };

            // Sanitize: only allow safe math characters
            let safe: String = expr.chars()
                .filter(|c| c.is_ascii_digit() || "+-*/%.() ^eE".contains(*c))
                .collect();

            if safe.is_empty() {
                return PluginResponse {
                    success: false,
                    result: Value::Null,
                    error: Some("Expression contains no valid math characters".into()),
                };
            }

            match evalexpr::eval(&safe) {
                Ok(val) => {
                    let numeric = match val {
                        evalexpr::Value::Float(f) => f,
                        evalexpr::Value::Int(i) => i as f64,
                        evalexpr::Value::Boolean(b) => if b { 1.0 } else { 0.0 },
                        other => {
                            return PluginResponse {
                                success: false,
                                result: Value::Null,
                                error: Some(format!("Unexpected result type: {:?}", other)),
                            }
                        }
                    };
                    PluginResponse {
                        success: true,
                        result: serde_json::json!({
                            "expression": safe,
                            "result":     numeric,
                            "result_str": format!("{}", numeric),
                        }),
                        error: None,
                    }
                }
                Err(e) => PluginResponse {
                    success: false,
                    result: Value::Null,
                    error: Some(format!("Evaluation error: {}", e)),
                },
            }
        }
        _ => PluginResponse {
            success: false,
            result: Value::Null,
            error: Some(format!(
                "Unknown action '{}'. Supported: calculate, eval, compute",
                req.action
            )),
        },
    }
}
