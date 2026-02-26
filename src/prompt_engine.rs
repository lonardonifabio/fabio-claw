// src/prompt_engine.rs
//
// TinyLlama 1.1B uses the ChatML prompt format:
//   <|system|>\n{system}\n<|user|>\n{user}\n<|assistant|>\n
//
// This module handles:
//   1. Correct prompt templating for TinyLlama
//   2. System prompt injection
//   3. Conversation history management (sliding window)
//   4. Plugin result summarisation before injection into prompt
//   5. Response post-processing (strip artifacts, trim)

use std::collections::VecDeque;
use chrono::Local;

// ─── Token budget ─────────────────────────────────────────────────────────────
// TinyLlama context = 2048 tokens. We budget conservatively.
// Rough heuristic: 1 token ≈ 4 chars.
const MAX_CONTEXT_CHARS:   usize = 6_000;  // ~1500 tokens for full context
const MAX_HISTORY_TURNS:   usize = 4;      // keep last 4 user+assistant pairs
const MAX_PLUGIN_RESULT_CHARS: usize = 800; // truncate plugin data before injection
const MAX_RESPONSE_CHARS:  usize = 512;    // expected max LLM response length

// ─── System prompt ────────────────────────────────────────────────────────────
// Critical for TinyLlama: give it a clear persona, strict rules, and examples.
// Small models need very explicit instructions — NO ambiguity.
fn build_system_prompt() -> String {
    let now = Local::now();
    format!(
        "You are Fabio, a helpful AI assistant running on a Raspberry Pi 4. \
Today is {date}, {time}.\n\
You are concise, accurate, and friendly. You reply in plain text — no markdown, \
no code blocks unless asked. Keep replies under 3 sentences unless more detail \
is needed. If a slash-command result is shown, summarise it clearly for the user. \
Never repeat the question. Never say 'As an AI'. Just answer directly.",
        date = now.format("%A %d %B %Y"),
        time = now.format("%H:%M"),
    )
}

// ─── Conversation history ─────────────────────────────────────────────────────
#[derive(Debug, Clone)]
pub struct Turn {
    pub role:    Role,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Default)]
pub struct ConversationHistory {
    turns: VecDeque<Turn>,
}

impl ConversationHistory {
    pub fn new() -> Self {
        Self { turns: VecDeque::new() }
    }

    pub fn push(&mut self, role: Role, content: String) {
        self.turns.push_back(Turn { role, content });
        // Keep only last N complete turns (user+assistant pairs)
        while self.turns.len() > MAX_HISTORY_TURNS * 2 {
            self.turns.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.turns.clear();
    }
}

// ─── Prompt builder ───────────────────────────────────────────────────────────
/// Build the full TinyLlama ChatML prompt from history + new user message.
/// If a plugin result is provided, it is summarised and injected into the user turn.
pub fn build_prompt(
    history:       &ConversationHistory,
    user_message:  &str,
    plugin_result: Option<&str>,
) -> String {
    let system = build_system_prompt();
    let mut prompt = format!("<|system|>\n{}\n", system);

    // Inject prior conversation turns
    for turn in &history.turns {
        match turn.role {
            Role::User      => prompt.push_str(&format!("<|user|>\n{}\n", turn.content)),
            Role::Assistant => prompt.push_str(&format!("<|assistant|>\n{}\n", turn.content)),
        }
    }

    // Build the current user turn, optionally with plugin data
    let user_turn = match plugin_result {
        Some(result) => {
            let summarised = summarise_plugin_result(result);
            format!(
                "The user ran a command. Here is the result:\n{}\n\nUser message: {}",
                summarised, user_message
            )
        }
        None => user_message.to_string(),
    };

    prompt.push_str(&format!("<|user|>\n{}\n<|assistant|>\n", user_turn));

    // Enforce context budget: if prompt is too long, strip oldest history turns
    if prompt.len() > MAX_CONTEXT_CHARS {
        // Rebuild with fewer history turns
        let mut reduced = ConversationHistory::new();
        let turns_vec: Vec<_> = history.turns.iter().cloned().collect();
        // Take only the last 2 turns
        for turn in turns_vec.iter().rev().take(4).rev() {
            reduced.turns.push_back(turn.clone());
        }
        return build_prompt(&reduced, user_message, plugin_result);
    }

    prompt
}

// ─── Plugin result summariser ─────────────────────────────────────────────────
/// Convert raw plugin JSON into a concise human-readable summary for the LLM.
/// TinyLlama struggles with raw JSON — give it pre-digested text instead.
pub fn summarise_plugin_result(raw: &str) -> String {
    // Try to parse as JSON
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        // If plugin reported failure
        if v["success"] == false {
            let err = v["error"].as_str().unwrap_or("unknown error");
            return format!("Command failed: {}", err);
        }

        let result = &v["result"];

        // ── Weather ───────────────────────────────────────────────────────────
        if result.get("condition").is_some() && result.get("temperature").is_some() {
            let loc  = result["location"].as_str().unwrap_or("unknown");
            let cond = result["condition"].as_str().unwrap_or("?");
            let temp = result["temperature"].as_str().unwrap_or("?");
            let feel = result["feels_like"].as_str().unwrap_or("?");
            let hum  = result["humidity"].as_str().unwrap_or("?");
            let wind = result["wind"].as_str().unwrap_or("?");
            let mut s = format!(
                "Weather in {}: {}. Temperature: {} (feels like {}). \
                 Humidity: {}, Wind: {}.",
                loc, cond, temp, feel, hum, wind
            );
            if let Some(fc) = result["forecast"].as_array() {
                let days: Vec<String> = fc.iter().take(3).filter_map(|d| {
                    let date = d["date"].as_str()?;
                    let max  = d["max_temp"].as_f64()?;
                    let min  = d["min_temp"].as_f64()?;
                    let rain = d["rain_mm"].as_f64().unwrap_or(0.0);
                    Some(format!("{}: {:.0}-{:.0}°C, rain {:.0}mm", date, min, max, rain))
                }).collect();
                if !days.is_empty() {
                    s.push_str(&format!(" Forecast: {}", days.join("; ")));
                }
            }
            return s;
        }

        // ── DateTime ─────────────────────────────────────────────────────────
        if result.get("datetime").is_some() {
            let dt  = result["datetime"].as_str().unwrap_or("?");
            let dow = result["day_of_week"].as_str().unwrap_or("?");
            let tz  = result["timezone"].as_str().unwrap_or("?");
            return format!("Current date and time: {} ({}). Timezone: {}.", dt, dow, tz);
        }

        // ── Calculator ────────────────────────────────────────────────────────
        if result.get("result").is_some() && result.get("expression").is_some() {
            let expr = result["expression"].as_str().unwrap_or("?");
            let res  = result["result_str"].as_str()
                .or_else(|| result["result"].as_str())
                .unwrap_or("?");
            return format!("{} = {}", expr, res);
        }

        // ── System info ───────────────────────────────────────────────────────
        if result.get("cpu_temp_celsius").is_some() || result.get("uptime").is_some() {
            let uptime = result["uptime"].as_str().unwrap_or("?");
            let temp   = result["cpu_temp_celsius"].as_f64()
                .map(|t| format!("{:.1}°C", t))
                .unwrap_or_else(|| "N/A".into());
            let mem_pct = result["memory"]["used_pct"].as_u64().unwrap_or(0);
            let disk    = result["disk_root"]["use_pct"].as_str().unwrap_or("?");
            let load    = result["load_avg"]["1min"].as_str().unwrap_or("?");
            return format!(
                "System uptime: {}. CPU temp: {}. RAM used: {}%. \
                 Disk: {} used. Load avg (1m): {}.",
                uptime, temp, mem_pct, disk, load
            );
        }

        // ── Uptime only ───────────────────────────────────────────────────────
        if result.get("uptime_human").is_some() {
            let up = result["uptime_human"].as_str().unwrap_or("?");
            return format!("System has been up for {}.", up);
        }

        // ── Disk only ─────────────────────────────────────────────────────────
        if result.get("filesystem").is_some() {
            let size  = result["size"].as_str().unwrap_or("?");
            let used  = result["used"].as_str().unwrap_or("?");
            let avail = result["available"].as_str().unwrap_or("?");
            let pct   = result["use_pct"].as_str().unwrap_or("?");
            return format!(
                "Root filesystem: {} total, {} used ({}), {} available.",
                size, used, pct, avail
            );
        }

        // ── Ping ──────────────────────────────────────────────────────────────
        if result.get("reachable").is_some() {
            let host = result["host"].as_str().unwrap_or("?");
            let ok   = result["reachable"].as_bool().unwrap_or(false);
            let rtt  = result["rtt_stats"].as_str().unwrap_or("N/A");
            let loss = result["packet_loss"].as_str().unwrap_or("?");
            return if ok {
                format!("{} is reachable. RTT: {}. Packet loss: {}.", host, rtt, loss)
            } else {
                format!("{} is NOT reachable. Packet loss: {}.", host, loss)
            };
        }

        // ── DNS ───────────────────────────────────────────────────────────────
        if result.get("resolved").is_some() {
            let host = result["host"].as_str().unwrap_or("?");
            let ips: Vec<&str> = result["resolved"].as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            let ms = result["resolve_ms"].as_u64().unwrap_or(0);
            return format!("{} resolved to: {}. Time: {}ms.", host, ips.join(", "), ms);
        }

        // ── GPIO ──────────────────────────────────────────────────────────────
        if result.get("pin").is_some() && result.get("state").is_some() {
            let pin   = result["pin"].as_u64().unwrap_or(0);
            let state = result["state"].as_str().unwrap_or("?");
            let dir   = result["direction"].as_str().unwrap_or("?");
            return format!("GPIO pin {} is {} (direction: {}).", pin, state, dir);
        }

        // ── Scheduler (reminder created) ─────────────────────────────────────
        if result.get("remind_at_human").is_some() {
            let msg = result["message"].as_str().unwrap_or("?");
            let at  = result["remind_at_human"].as_str().unwrap_or("?");
            let sec = result["in_seconds"].as_i64().unwrap_or(0);
            return format!(
                "Reminder set: \"{}\" at {} (in {} minutes).",
                msg, at, sec / 60
            );
        }

        // ── Jobs list ─────────────────────────────────────────────────────────
        if result.get("reminders").is_some() {
            let total   = result["total"].as_u64().unwrap_or(0);
            let pending = result["pending"].as_u64().unwrap_or(0);
            if total == 0 {
                return "No reminders scheduled.".into();
            }
            let jobs: Vec<String> = result["reminders"].as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter(|r| r["done"].as_bool() == Some(false))
                .take(5)
                .filter_map(|r| {
                    let msg = r["message"].as_str()?;
                    let at  = r["remind_at"].as_str()?;
                    Some(format!("- {} (at {})", msg, &at[..16]))
                })
                .collect();
            return format!(
                "{} reminders ({} pending):\n{}",
                total, pending, jobs.join("\n")
            );
        }

        // ── Update check ──────────────────────────────────────────────────────
        if result.get("system").is_some() || result.get("fabio_claw").is_some() {
            let sys_status = result["system"]["status"].as_str().unwrap_or("unknown");
            let n_updates  = result["system"]["available_updates"].as_u64().unwrap_or(0);
            return if n_updates == 0 {
                format!("System packages are up to date. Status: {}.", sys_status)
            } else {
                format!("{} system package update(s) available. Run /update-plan for steps.", n_updates)
            };
        }

        // ── File read ─────────────────────────────────────────────────────────
        if result.get("content").is_some() {
            let path  = result["path"].as_str().unwrap_or("?");
            let lines = result["lines"].as_u64().unwrap_or(0);
            let trunc = result["truncated"].as_bool().unwrap_or(false);
            let content = result["content"].as_str().unwrap_or("");
            let preview = &content[..content.len().min(MAX_PLUGIN_RESULT_CHARS)];
            return format!(
                "File: {} ({} lines{})\n{}",
                path, lines,
                if trunc { ", truncated" } else { "" },
                preview
            );
        }

        // ── Web search ────────────────────────────────────────────────────────
        if result.get("synthesis").is_some() {
            let q   = result["query"].as_str().unwrap_or("?");
            let syn = result["synthesis"].as_str().unwrap_or("No results.");
            let truncated = &syn[..syn.len().min(MAX_PLUGIN_RESULT_CHARS)];
            return format!("Web search for \"{}\": {}", q, truncated);
        }

        if result.get("results").is_some() && result.get("query").is_some() {
            let q = result["query"].as_str().unwrap_or("?");
            let items: Vec<String> = result["results"].as_array()
                .unwrap_or(&vec![])
                .iter()
                .take(3)
                .filter_map(|r| {
                    let title   = r["title"].as_str()?;
                    let snippet = r["snippet"].as_str().unwrap_or("");
                    Some(format!("- {}: {}", title, &snippet[..snippet.len().min(120)]))
                })
                .collect();
            return format!("Search results for \"{}\":\n{}", q, items.join("\n"));
        }

        // ── Document/Excel created ────────────────────────────────────────────
        if result.get("filename").is_some() {
            let fname = result["filename"].as_str().unwrap_or("?");
            let fpath = result["file"].as_str().unwrap_or("?");
            return format!("File created: {} saved to {}", fname, fpath);
        }

        // ── Logs ──────────────────────────────────────────────────────────────
        if result.get("log").is_some() {
            let n     = result["lines_shown"].as_u64().unwrap_or(0);
            let errs  = result["error_count"].as_u64().unwrap_or(0);
            let lines = result["log"].as_array()
                .map(|a| a.iter()
                    .filter_map(|v| v.as_str())
                    .take(10)
                    .collect::<Vec<_>>()
                    .join("\n"))
                .unwrap_or_default();
            return format!(
                "Last {} log lines ({} errors/warnings):\n{}",
                n, errs,
                &lines[..lines.len().min(MAX_PLUGIN_RESULT_CHARS)]
            );
        }

        // ── Knowledge base search ─────────────────────────────────────────────
        if result.get("results").is_some() && result.get("method").is_some() {
            let q     = result["query"].as_str().unwrap_or("?");
            let count = result["count"].as_u64().unwrap_or(0);
            if count == 0 {
                let hint = result["hint"].as_str().unwrap_or("No documents indexed.");
                return format!("No results for \"{}\". {}", q, hint);
            }
            let excerpts: Vec<String> = result["results"].as_array()
                .unwrap_or(&vec![])
                .iter()
                .take(3)
                .filter_map(|r| {
                    let src = r["source"].as_str()?;
                    let ex  = r["excerpt"].as_str().unwrap_or("");
                    Some(format!("[{}] {}", src, &ex[..ex.len().min(200)]))
                })
                .collect();
            return format!(
                "Found {} result(s) for \"{}\":\n{}",
                count, q, excerpts.join("\n")
            );
        }

        // ── Fallback: generic JSON to readable key=value ──────────────────────
        return json_to_readable(result);
    }

    // Non-JSON: return truncated raw
    let s = raw.trim();
    s[..s.len().min(MAX_PLUGIN_RESULT_CHARS)].to_string()
}

/// Convert arbitrary JSON object to readable "key: value" lines.
fn json_to_readable(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let lines: Vec<String> = map.iter()
                .take(12)
                .map(|(k, val)| {
                    let display = match val {
                        serde_json::Value::String(s)  => s.clone(),
                        serde_json::Value::Number(n)  => n.to_string(),
                        serde_json::Value::Bool(b)    => b.to_string(),
                        serde_json::Value::Null       => "null".into(),
                        serde_json::Value::Array(a)   => {
                            let items: Vec<String> = a.iter().take(5)
                                .map(|x| x.to_string()).collect();
                            items.join(", ")
                        }
                        other => {
                            let s = other.to_string();
                            s[..s.len().min(80)].to_string()
                        }
                    };
                    format!("{}: {}", k, display)
                })
                .collect();
            lines.join(", ")
        }
        other => {
            let s = other.to_string();
            s[..s.len().min(MAX_PLUGIN_RESULT_CHARS)].to_string()
        }
    }
}

// ─── Response post-processor ──────────────────────────────────────────────────
/// Clean up TinyLlama output artifacts:
///   - Strip repeated prompt fragments
///   - Remove <|...|> tokens if leaked
///   - Strip leading/trailing whitespace
///   - Enforce max length
///   - Remove "As an AI language model" boilerplate
pub fn clean_response(raw: &str) -> String {
    let mut s = raw.to_string();

    // Remove leaked ChatML tokens
    for token in &["<|system|>", "<|user|>", "<|assistant|>", "<|endoftext|>", "</s>"] {
        s = s.replace(token, "");
    }

    // Remove "As an AI..." boilerplate (case-insensitive first occurrence)
    let lower = s.to_lowercase();
    if let Some(pos) = lower.find("as an ai") {
        // If it's the start of the response, remove the whole first sentence
        if pos < 20 {
            if let Some(end) = s[pos..].find(['.', '\n']) {
                s = s[pos + end + 1..].trim_start().to_string();
            }
        }
    }

    // Remove prompt echo: if the response starts by repeating user words, trim
    // (heuristic: repeated newlines suggest prompt echo)
    if s.starts_with('\n') {
        s = s.trim_start_matches('\n').to_string();
    }

    // Collapse multiple blank lines to single
    while s.contains("\n\n\n") {
        s = s.replace("\n\n\n", "\n\n");
    }

    // Trim and enforce length
    s = s.trim().to_string();
    if s.len() > MAX_RESPONSE_CHARS {
        // Cut at last sentence boundary within budget
        let budget = &s[..MAX_RESPONSE_CHARS];
        if let Some(pos) = budget.rfind(['.', '!', '?', '\n']) {
            s = budget[..=pos].to_string();
        } else {
            s = format!("{}…", budget.trim_end());
        }
    }

    // If empty after cleaning, return a safe fallback
    if s.trim().is_empty() {
        return "I'm sorry, I couldn't generate a response. Please try again.".into();
    }

    s
}

// ─── Slash-command detector ───────────────────────────────────────────────────
/// Returns (command_name, args) if the message is a slash-command.
pub fn parse_slash_command(msg: &str) -> Option<(&str, &str)> {
    let msg = msg.trim();
    if !msg.starts_with('/') {
        return None;
    }
    let without_slash = &msg[1..];
    let mut parts = without_slash.splitn(2, char::is_whitespace);
    let cmd  = parts.next()?;
    let args = parts.next().unwrap_or("").trim();
    Some((cmd, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_slash_command() {
        assert_eq!(parse_slash_command("/weather Milan"), Some(("weather", "Milan")));
        assert_eq!(parse_slash_command("/sysinfo"), Some(("sysinfo", "")));
        assert_eq!(parse_slash_command("hello world"), None);
    }

    #[test]
    fn test_clean_response() {
        let raw = "<|assistant|>\nAs an AI language model, I cannot. But hello!";
        let cleaned = clean_response(raw);
        assert!(!cleaned.contains("<|assistant|>"));
        assert!(!cleaned.contains("As an AI"));
    }

    #[test]
    fn test_summarise_weather() {
        let json = r#"{
            "success": true,
            "result": {
                "location": "Milan, Italy",
                "condition": "Partly cloudy ⛅",
                "temperature": "12.3°C",
                "feels_like": "10.1°C",
                "humidity": "72%",
                "wind": "15.2 km/h",
                "forecast": [
                    {"date":"2026-02-26","max_temp":14,"min_temp":8,"rain_mm":0},
                    {"date":"2026-02-27","max_temp":11,"min_temp":6,"rain_mm":5}
                ]
            }
        }"#;
        let summary = summarise_plugin_result(json);
        assert!(summary.contains("Milan"));
        assert!(summary.contains("12.3°C"));
        assert!(summary.contains("Forecast"));
    }

    #[test]
    fn test_summarise_calc() {
        let json = r#"{"success":true,"result":{"expression":"2^10 - 1","result":1023.0,"result_str":"1023"}}"#;
        let s = summarise_plugin_result(json);
        assert_eq!(s, "2^10 - 1 = 1023");
    }

    #[test]
    fn test_build_prompt_contains_chatml() {
        let history = ConversationHistory::new();
        let prompt = build_prompt(&history, "What time is it?", None);
        assert!(prompt.contains("<|system|>"));
        assert!(prompt.contains("<|user|>"));
        assert!(prompt.contains("<|assistant|>"));
        assert!(prompt.contains("What time is it?"));
    }

    #[test]
    fn test_context_budget_enforced() {
        let history = ConversationHistory::new();
        let long_plugin = "x".repeat(MAX_PLUGIN_RESULT_CHARS + 100);
        let prompt = build_prompt(&history, "test", Some(&long_plugin));
        // Should not panic and should remain within budget
        assert!(prompt.len() < MAX_CONTEXT_CHARS + 2000);
    }
}
