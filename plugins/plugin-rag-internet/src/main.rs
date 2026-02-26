use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};

#[derive(Debug, Deserialize)]
struct PluginRequest { action: String, payload: Value }
#[derive(Debug, Serialize)]
struct PluginResponse { success: bool, result: Value, error: Option<String> }

fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap_or(0);
    let response = match serde_json::from_str::<PluginRequest>(&input) {
        Ok(req) => handle(req),
        Err(e)  => PluginResponse { success: false, result: Value::Null,
            error: Some(format!("Invalid request JSON: {}", e)) },
    };
    println!("{}", serde_json::to_string(&response).unwrap());
}

fn handle(req: PluginRequest) -> PluginResponse {
    let args = req.payload.get("args").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if args.is_empty() {
        return err("Provide a search query. Usage: /web-search <query>".into());
    }
    match req.action.as_str() {
        "rag" | "web-rag" => web_rag(&args),
        _                 => web_search(&args),
    }
}

fn web_search(query: &str) -> PluginResponse {
    let results = ddg_search(query);
    PluginResponse { success: true, result: serde_json::json!({
        "query": query, "source": "DuckDuckGo",
        "count": results.len(), "results": results
    }), error: None }
}

fn web_rag(query: &str) -> PluginResponse {
    let results = ddg_search(query);
    let synthesis = if results.is_empty() { "No results found.".to_string() } else {
        results.iter().take(3).enumerate().filter_map(|(i, r)| {
            let snippet = r["snippet"].as_str().unwrap_or("");
            let title   = r["title"].as_str().unwrap_or("");
            let url     = r["url"].as_str().unwrap_or("");
            if snippet.is_empty() { return None; }
            Some(format!("[{}] {} — {} ({})", i + 1, title, snippet, url))
        }).collect::<Vec<_>>().join("\n\n")
    };
    let citations: Vec<Value> = results.iter().map(|r| {
        serde_json::json!({ "title": r["title"], "url": r["url"] })
    }).collect();
    PluginResponse { success: true, result: serde_json::json!({
        "query": query, "synthesis": synthesis,
        "citations": citations, "source": "DuckDuckGo"
    }), error: None }
}

fn ddg_search(query: &str) -> Vec<Value> {
    let encoded = url_encode(query);
    let url = format!(
        "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1", encoded);
    let body: Value = match ureq::get(&url)
        .set("User-Agent", "fabio-claw/0.3.0").call() {
        Ok(r)  => match r.into_json() {
            Ok(j)  => j,
            Err(e) => return vec![serde_json::json!({"error": format!("{}", e)})],
        },
        Err(e) => return vec![serde_json::json!({
            "error": format!("Request failed: {}", e),
            "fallback_url": format!("https://duckduckgo.com/?q={}", encoded)
        })],
    };
    let mut results: Vec<Value> = vec![];
    if let Some(a) = body["Answer"].as_str() { if !a.is_empty() {
        results.push(serde_json::json!({
            "title": "Direct Answer", "snippet": a, "url": "", "type": "answer_box"
        }));
    }}
    if let Some(a) = body["Abstract"].as_str() { if !a.is_empty() {
        results.push(serde_json::json!({
            "title":   body["Heading"].as_str().unwrap_or("Summary"),
            "snippet": a,
            "url":     body["AbstractURL"].as_str().unwrap_or(""),
            "source":  body["AbstractSource"].as_str().unwrap_or(""),
            "type":    "abstract"
        }));
    }}
    if let Some(topics) = body["RelatedTopics"].as_array() {
        for t in topics.iter().take(5) {
            let text = t["Text"].as_str().unwrap_or_default();
            let url  = t["FirstURL"].as_str().unwrap_or_default();
            if !text.is_empty() {
                results.push(serde_json::json!({
                    "title":   &text[..text.len().min(80)],
                    "snippet": &text[..text.len().min(200)],
                    "url": url, "type": "related"
                }));
            }
        }
    }
    if results.is_empty() {
        results.push(serde_json::json!({
            "title":   "No instant results",
            "snippet": "Try a more specific query.",
            "url":     format!("https://duckduckgo.com/?q={}", encoded),
            "type":    "no_results"
        }));
    }
    results
}

fn url_encode(s: &str) -> String {
    s.chars().map(|c| {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') { c.to_string() }
        else if c == ' ' { "+".to_string() }
        else { format!("%{:02X}", c as u8) }
    }).collect()
}

fn err(msg: String) -> PluginResponse {
    PluginResponse { success: false, result: Value::Null, error: Some(msg) }
}
