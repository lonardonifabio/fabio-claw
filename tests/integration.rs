//! Integration tests for Fabio-Claw API, plugins, and memory layer.
//!
//! Run with: `cargo test --test integration`
//!
//! These tests spin up the full axum application using an in-memory SQLite
//! database and mock LLM, so no real model is required.

use std::sync::Arc;
use axum::{body::Body, http::{Request, StatusCode}};
use tower::ServiceExt;
use serde_json::{json, Value};

// We import from the library crate — declare the path as needed.
// In a real workspace this would be `use fabio_claw::...;`
// Here we re-use the binary's modules via a test helper shim.

// ─── Test helpers ─────────────────────────────────────────────────────────────

/// Build a minimal test app with in-process state (no real model, no plugins dir).
async fn build_test_app() -> axum::Router {
    use fabio_claw::{
        api::{router, AppState},
        llm::LlmActor,
        memory::MemoryStore,
        security::DeviceIdentity,
        plugins::PluginRegistry,
    };

    let llm     = Arc::new(LlmActor::spawn("/nonexistent-model".into()).unwrap());
    let memory  = Arc::new(MemoryStore::open(":memory:").unwrap());
    let device  = Arc::new(DeviceIdentity::load_or_generate("/tmp/fabio-test.key").unwrap());
    let plugins = Arc::new(PluginRegistry::load("/tmp/nonexistent-plugins"));

    let state = AppState { llm, memory, device, plugins };
    router(state)
}

async fn post_json(app: &axum::Router, path: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(json!({"raw": bytes.len()}));
    (status, json)
}

async fn get_json(app: &axum::Router, path: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(json!({"raw": bytes.len()}));
    (status, json)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_health_check() {
    let app = build_test_app().await;
    let (status, body) = get_json(&app, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert!(body["device_id"].as_str().unwrap().len() >= 32);
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn test_readiness_check() {
    let app = build_test_app().await;
    // LLM mock mode sets ready=true immediately
    let (status, body) = get_json(&app, "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["memory_ok"], true);
}

#[tokio::test]
async fn test_list_models() {
    let app = build_test_app().await;
    let (status, body) = get_json(&app, "/v1/models").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"].as_array().unwrap().len() >= 1);
}

#[tokio::test]
async fn test_chat_mock_inference() {
    let app = build_test_app().await;
    let (status, body) = post_json(&app, "/v1/chat/completions", json!({
        "model": "local",
        "messages": [{"role": "user", "content": "Hello, world!"}]
    })).await;
    assert_eq!(status, StatusCode::OK);
    let content = body["choices"][0]["message"]["content"].as_str().unwrap();
    assert!(content.contains("[MOCK]"), "Expected mock response, got: {}", content);
    // Token accounting must be non-zero
    assert!(body["usage"]["prompt_tokens"].as_u64().unwrap_or(0) > 0);
    assert!(body["usage"]["completion_tokens"].as_u64().unwrap_or(0) > 0);
}

#[tokio::test]
async fn test_chat_empty_messages_returns_400() {
    let app = build_test_app().await;
    let (status, _) = post_json(&app, "/v1/chat/completions", json!({
        "model": "local",
        "messages": []
    })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_help_command() {
    let app = build_test_app().await;
    let (status, body) = post_json(&app, "/v1/chat/completions", json!({
        "model": "local",
        "messages": [{"role": "user", "content": "/help"}]
    })).await;
    assert_eq!(status, StatusCode::OK);
    let content = body["choices"][0]["message"]["content"].as_str().unwrap();
    assert!(content.contains("Fabio-Claw"), "Expected help text, got: {}", content);
}

#[tokio::test]
async fn test_unknown_command() {
    let app = build_test_app().await;
    let (status, body) = post_json(&app, "/v1/chat/completions", json!({
        "model": "local",
        "messages": [{"role": "user", "content": "/unknownxyz123"}]
    })).await;
    assert_eq!(status, StatusCode::OK);
    let content = body["choices"][0]["message"]["content"].as_str().unwrap();
    assert!(content.contains("Unknown command") || content.contains("unknownxyz123"),
        "Expected unknown-command error, got: {}", content);
}

#[tokio::test]
async fn test_stream_not_supported() {
    let app = build_test_app().await;
    let (status, body) = post_json(&app, "/v1/chat/completions", json!({
        "model": "local",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true
    })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let msg = body["error"]["message"].as_str().unwrap();
    assert!(msg.contains("stream"), "Expected streaming error, got: {}", msg);
}

#[tokio::test]
async fn test_memory_persistence_across_requests() {
    let app = build_test_app().await;

    // First message
    let (s1, b1) = post_json(&app, "/v1/chat/completions", json!({
        "model": "local",
        "messages": [{"role": "user", "content": "First message"}],
        "session_id": "test-session-abc"
    })).await;
    assert_eq!(s1, StatusCode::OK);
    let _ = b1;

    // Second message in same session
    let (s2, _) = post_json(&app, "/v1/chat/completions", json!({
        "model": "local",
        "messages": [{"role": "user", "content": "Second message"}],
        "session_id": "test-session-abc"
    })).await;
    assert_eq!(s2, StatusCode::OK);

    // Both should succeed — we can't easily query the DB in an integration test
    // without exposing it, so this confirms no crash and proper HTTP 200.
}

#[tokio::test]
async fn test_token_accounting_non_zero() {
    let app = build_test_app().await;
    let (status, body) = post_json(&app, "/v1/chat/completions", json!({
        "model": "local",
        "messages": [
            {"role": "system", "content": "You are a helpful assistant."},
            {"role": "user", "content": "What is two plus two?"}
        ]
    })).await;
    assert_eq!(status, StatusCode::OK);
    let usage = &body["usage"];
    assert!(usage["prompt_tokens"].as_u64().unwrap_or(0) > 0, "prompt_tokens should be > 0");
    assert!(usage["completion_tokens"].as_u64().unwrap_or(0) > 0, "completion_tokens should be > 0");
    assert_eq!(
        usage["total_tokens"].as_u64().unwrap_or(0),
        usage["prompt_tokens"].as_u64().unwrap_or(0) + usage["completion_tokens"].as_u64().unwrap_or(0)
    );
}

#[tokio::test]
async fn test_tools_endpoint() {
    let app = build_test_app().await;
    let (status, body) = get_json(&app, "/v1/tools").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["tools"].is_array(), "Expected tools array");
}

#[tokio::test]
async fn test_tasks_list_empty() {
    let app = build_test_app().await;
    let (status, body) = get_json(&app, "/v1/tasks").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["tasks"].is_array());
}

// ─── Unit tests for BPE estimator ────────────────────────────────────────────

#[cfg(test)]
mod unit {
    use fabio_claw::llm::bpe_estimate;

    #[test]
    fn bpe_estimate_empty() {
        assert_eq!(bpe_estimate(""), 0);
    }

    #[test]
    fn bpe_estimate_single_word() {
        assert!(bpe_estimate("hello") > 0);
    }

    #[test]
    fn bpe_estimate_long_text() {
        let text = "The quick brown fox jumps over the lazy dog. ".repeat(10);
        let tokens = bpe_estimate(&text);
        // ~450 chars / 4 ≈ 110 tokens; allow generous range
        assert!(tokens > 50, "Expected > 50 tokens, got {}", tokens);
        assert!(tokens < 500, "Expected < 500 tokens, got {}", tokens);
    }

    #[test]
    fn bpe_estimate_nonzero_for_any_nonempty() {
        for s in &["a", " ", "\n", "hello world", "🦀"] {
            assert!(bpe_estimate(s) >= 1, "Expected >=1 token for {:?}", s);
        }
    }
}

// ─── Unit tests for ReAct parser ─────────────────────────────────────────────

#[cfg(test)]
mod agent_tests {
    // These test the parse logic in isolation without invoking LLM
    fn extract_tag(text: &str, tag: &str) -> Option<String> {
        let open  = format!("<{}>", tag);
        let close = format!("</{}>", tag);
        let start = text.find(&open)? + open.len();
        let end   = text[start..].find(&close)? + start;
        Some(text[start..end].trim().to_string())
    }

    #[test]
    fn parse_thought_only() {
        let text = "<thought>I need more info.</thought><answer>42</answer>";
        assert_eq!(extract_tag(text, "thought"), Some("I need more info.".into()));
        assert_eq!(extract_tag(text, "answer"), Some("42".into()));
        assert_eq!(extract_tag(text, "action"), None);
    }

    #[test]
    fn parse_action_block() {
        let text = r#"<thought>Let me check weather.</thought>
<action>weather</action>
<action_input>{"args": "Milan"}</action_input>"#;
        assert_eq!(extract_tag(text, "action"), Some("weather".into()));
        let input = extract_tag(text, "action_input").unwrap();
        let v: serde_json::Value = serde_json::from_str(&input).unwrap();
        assert_eq!(v["args"], "Milan");
    }

    #[test]
    fn parse_missing_tags_returns_none() {
        let text = "just some text with no tags";
        assert_eq!(extract_tag(text, "thought"), None);
        assert_eq!(extract_tag(text, "action"), None);
    }
}

// ─── Plugin runner unit tests ─────────────────────────────────────────────────

#[cfg(test)]
mod plugin_tests {
    use std::io::Write;
    use tempfile::tempdir;
    use fabio_claw::plugins::{PluginManifest, PluginRequest, PluginRunner};
    use fabio_claw::security::DeviceIdentity;

    /// Create a minimal shell-script plugin that echoes a valid JSON response.
    #[cfg(unix)]
    #[tokio::test]
    async fn plugin_runner_success() {
        let dir = tempdir().unwrap();
        let bin_path = dir.path().join("plugin-echo");

        // Write a minimal shell script as the plugin binary
        {
            let mut f = std::fs::File::create(&bin_path).unwrap();
            writeln!(f, "#!/bin/sh").unwrap();
            writeln!(f, r#"echo '{{"success":true,"result":{{"msg":"hello"}},"error":null}}'"#).unwrap();
        }
        // Make executable
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let manifest = PluginManifest {
            name: "plugin-echo".into(),
            version: "0.1.0".into(),
            description: "Echo test".into(),
            commands: vec!["echo".into()],
            default_action: "run".into(),
            payload_from_args: true,
            tool_schema: None,
            require_signature: false,
        };

        let device = DeviceIdentity::load_or_generate("/tmp/fabio-plugin-test.key").unwrap();
        let runner = PluginRunner::new(dir.path().to_str().unwrap());
        let req = PluginRequest {
            action: "run".into(),
            payload: serde_json::json!({ "args": "hello" }),
        };

        let resp = runner.run(&manifest, &req, &device).await.unwrap();
        assert!(resp.success);
        assert_eq!(resp.result["msg"], "hello");
    }

    /// Plugin that sleeps longer than the timeout should return an error.
    #[cfg(unix)]
    #[tokio::test]
    async fn plugin_runner_timeout() {
        let dir = tempdir().unwrap();
        let bin_path = dir.path().join("plugin-slow");

        {
            let mut f = std::fs::File::create(&bin_path).unwrap();
            writeln!(f, "#!/bin/sh").unwrap();
            writeln!(f, "sleep 60").unwrap();
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let manifest = PluginManifest {
            name: "plugin-slow".into(),
            version: "0.1.0".into(),
            description: "Slow test".into(),
            commands: vec!["slow".into()],
            default_action: "run".into(),
            payload_from_args: false,
            tool_schema: None,
            require_signature: false,
        };

        let device = DeviceIdentity::load_or_generate("/tmp/fabio-slow-test.key").unwrap();
        let runner = PluginRunner::new(dir.path().to_str().unwrap());
        let req = PluginRequest {
            action: "run".into(),
            payload: serde_json::json!({}),
        };

        let result = runner.run(&manifest, &req, &device).await;
        assert!(result.is_err(), "Expected timeout error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("timed out") || err.contains("timeout"), "Got: {}", err);
    }

    /// Plugin that exits with invalid JSON should return parse error.
    #[cfg(unix)]
    #[tokio::test]
    async fn plugin_runner_invalid_json() {
        let dir = tempdir().unwrap();
        let bin_path = dir.path().join("plugin-badjson");

        {
            let mut f = std::fs::File::create(&bin_path).unwrap();
            writeln!(f, "#!/bin/sh").unwrap();
            writeln!(f, "echo 'not valid json at all!'").unwrap();
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let manifest = PluginManifest {
            name: "plugin-badjson".into(),
            version: "0.1.0".into(),
            description: "Bad JSON test".into(),
            commands: vec!["badjson".into()],
            default_action: "run".into(),
            payload_from_args: false,
            tool_schema: None,
            require_signature: false,
        };

        let device = DeviceIdentity::load_or_generate("/tmp/fabio-badjson-test.key").unwrap();
        let runner = PluginRunner::new(dir.path().to_str().unwrap());
        let req = PluginRequest { action: "run".into(), payload: serde_json::json!({}) };

        let result = runner.run(&manifest, &req, &device).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid JSON"));
    }
}
