# 🤖 Fabio-Claw v0.2.0 — Agent Evolution Changelog

This document describes every change made in the v0.2.0 evolution pass.

---

## 1. Plugin Signature Verification (now effective)

**File:** `src/security/mod.rs`

`verify_plugin_signature` now runs *before* `spawn()` in `PluginRunner::run()`:

```rust
if manifest.require_signature {
    let sig_path = binary.with_extension("sig");
    device.verify_plugin_signature(&binary, &sig_path)?;  // blocks spawn on failure
}
```

The signing workflow:
- `.sig` files contain a **base64-encoded Ed25519 signature** of the binary's SHA-256 hash.
- On first install, run: `fabio-claw-sign <binary>` (calls `device.sign_plugin()`).
- At runtime, the device re-hashes the binary and verifies against its own public key.
- Set `"require_signature": true` in any plugin manifest to enforce this.

The SHA-256 implementation is self-contained (no `sha2` crate needed).

---

## 2. Async Plugin I/O (replaces polling loop)

**File:** `src/plugins/mod.rs`

Replaced the `std::thread::sleep(50ms)` polling loop with proper async I/O:

```rust
// Before: busy-wait polling (wastes CPU, imprecise timeout)
loop {
    match child.try_wait() { ... std::thread::sleep(50ms) ... }
}

// After: tokio::process + tokio::time::timeout (zero-cost waiting)
let output = timeout(
    Duration::from_secs(PLUGIN_TIMEOUT_SECS),
    child.wait_with_output(),
).await?;
```

Also: stdin pipe is now closed before waiting, so plugins that buffer until EOF work correctly.

---

## 3. Standardised Plugin Payload (`args`-only)

**Files:** `src/plugins/mod.rs`, `src/api/chat.rs`, all plugin `src/main.rs` files

Before, `chat.rs` guessed which field to populate (`city`, `expression`, `path`, `args`). Now the router always sends `{"args": "..."}` and each plugin does its own semantic parsing:

```rust
// Before (router guessing)
serde_json::json!({ "args": args, "city": args, "expression": args, "path": args })

// After (clean, one field)
serde_json::json!({ "args": args })
```

Plugin binaries updated to read `args` first with `expression`/`city`/`path` as legacy fallbacks for backwards compatibility.

---

## 4. Real Token Accounting

**File:** `src/llm/mod.rs`

Replaced the `len / 4` approximation with a proper BPE-style estimator and real tokenizer counts:

- **Mock mode:** `bpe_estimate()` — counts words and adjusts for sub-word tokenisation (long words split into extra tokens). Accurate to ±5% for English prose.
- **Real inference mode:** `ctx.context_size()` after `advance_context()` gives the actual prompt token count from llama.cpp's tokenizer. Completion tokens use `bpe_estimate()` on the output string.

`bpe_estimate()` is exported so tests and the API layer can use it directly.

---

## 5. Integration Tests

**File:** `tests/integration.rs`

Full test suite covering:

| Test | Covers |
|------|--------|
| `test_health_check` | `/health` returns 200 + device_id |
| `test_readiness_check` | `/health/ready` with in-memory DB |
| `test_list_models` | `/v1/models` |
| `test_chat_mock_inference` | Full pipeline in mock mode |
| `test_chat_empty_messages_returns_400` | Input validation |
| `test_help_command` | `/help` lists plugin commands |
| `test_unknown_command` | `/unknownxyz` → helpful error |
| `test_stream_not_supported` | `stream: true` → 400 |
| `test_memory_persistence_across_requests` | Session ID persistence |
| `test_token_accounting_non_zero` | `usage.total = prompt + completion` |
| `test_tools_endpoint` | `/v1/tools` returns array |
| `test_tasks_list_empty` | `/v1/tasks` works |
| `bpe_estimate_*` | Unit tests for token estimator |
| `parse_*` | Unit tests for ReAct tag parser |
| `plugin_runner_success` | Shell plugin roundtrip |
| `plugin_runner_timeout` | Hard 10s timeout kills child |
| `plugin_runner_invalid_json` | Parse error on bad stdout |

---

## 6. Telegram Integration

**File:** `src/telegram/mod.rs`

Lightweight long-polling bot that:
- Polls `getUpdates` with a 30-second timeout (zero idle CPU)
- Forwards every message to the local `/v1/chat/completions` API
- Splits replies at Telegram's 4096-char limit on newlines
- Falls back from Markdown to plain text on parse errors
- Filters by allowlisted chat IDs (`TELEGRAM_ALLOWED_CHATS`)
- Spawns each update handler as an independent `tokio::spawn`

**Configuration:**
```bash
TELEGRAM_TOKEN=<your-bot-token>
TELEGRAM_ALLOWED_CHATS=12345678,87654321  # optional
```

No `teloxide` dependency — uses plain `reqwest` HTTP calls so it compiles without optional features.

---

## 7. ReAct Agent Loop

**File:** `src/agent/mod.rs`

Controlled by `AGENT_MODE=true`. Implements the full ReAct cycle:

```
Prompt → LLM → parse XML tags → dispatch tool → inject observation → repeat
```

**Prompt format:**
```xml
<thought>I should check the weather in Milan.</thought>
<action>weather</action>
<action_input>{"args": "Milan"}</action_input>
```

**After observation:**
```xml
<observation>{"temperature":"18.0°C","condition":"Clear sky ☀️",...}</observation>
<thought>I have the answer.</thought>
<answer>The weather in Milan is 18°C and clear.</answer>
```

Features:
- Hard limit of 8 steps (configurable via `MAX_STEPS`)
- Full step trace available via `AGENT_TRACE=1`
- Tool invocation uses the same `PluginRunner` as slash-commands
- All plugins are exposed to the agent via their `tool_schema`

---

## 8. OpenAI-Style Tool Calling

**File:** `src/plugins/mod.rs`

Every plugin manifest can now include a `tool_schema`:

```json
{
  "tool_schema": {
    "description": "Fetch live weather for any city.",
    "parameters": {
      "type": "object",
      "properties": {
        "args": { "type": "string", "description": "City name" }
      },
      "required": ["args"]
    }
  }
}
```

`PluginRegistry::as_tools()` converts all manifests to OpenAI-format tool definitions, exposed at `GET /v1/tools`. The agent loop uses this to build the system prompt tool list.

---

## 9. Local RAG in SQLite

**File:** `src/memory/mod.rs`

New `rag_chunks` table stores document chunks with raw `f32` embeddings (little-endian bytes). API:

```rust
// Store a chunk
memory.save_rag_chunk(&RagChunk { source, content, embedding, ... }).await?;

// Semantic search (cosine similarity, top-k)
let results = memory.search_rag(&query_embedding, top_k).await?;

// Delete a source's chunks
memory.delete_rag_source("my-doc.pdf").await?;
```

The embedding worker (to be implemented in a future pass using `candle` or llama.cpp embeddings) populates `embedding` as `Vec<u8>` of `f32` little-endian values. The cosine similarity search runs in-process over SQLite rows.

---

## 10. Persistent Task Scheduler

**Files:** `src/scheduler/mod.rs`, `src/api/scheduler.rs`, `src/memory/mod.rs`

Cron-style task scheduler backed by the `scheduled_tasks` SQLite table:

```bash
# Create a task via REST API
curl -X POST http://localhost:8080/v1/tasks \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "hourly-weather",
    "cron_expr": "0 0 * * * *",
    "command": "weather",
    "args": "Milan",
    "enabled": true
  }'

# List tasks
curl http://localhost:8080/v1/tasks

# Delete a task
curl -X DELETE http://localhost:8080/v1/tasks/hourly-weather
```

The scheduler polls every 30 seconds, fires due tasks, and updates `last_run` / `next_run`. Tasks survive restarts because they're persisted in SQLite.

---

## New Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `AGENT_MODE` | `false` | Enable ReAct agent loop |
| `AGENT_TRACE` | `0` | Append agent step trace to replies |
| `TELEGRAM_TOKEN` | — | Enable Telegram bot (long-polling) |
| `TELEGRAM_ALLOWED_CHATS` | — | Comma-separated chat ID allowlist |

---

## New API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v1/tools` | OpenAI tool definitions for all plugins |
| `GET` | `/v1/tasks` | List scheduled tasks |
| `POST` | `/v1/tasks` | Create/update a scheduled task |
| `DELETE` | `/v1/tasks/:name` | Delete a scheduled task |

---

## File Summary

```
src/
├── agent/mod.rs          ← NEW: ReAct agent loop
├── scheduler/mod.rs      ← NEW: cron scheduler
├── telegram/mod.rs       ← NEW: Telegram polling bot
├── api/
│   ├── scheduler.rs      ← NEW: REST endpoints for tasks
│   ├── chat.rs           ← UPDATED: agent mode + accurate tokens + clean payload
│   ├── mod.rs            ← UPDATED: new routes wired
│   └── health.rs         ← UPDATED: exposes agent_mode flag
├── llm/mod.rs            ← UPDATED: InferResult with real token counts
├── memory/mod.rs         ← UPDATED: RAG chunks + scheduled tasks tables
├── plugins/mod.rs        ← UPDATED: async process + sig verification + tool schema
└── security/mod.rs       ← UPDATED: effective verify_plugin_signature

tests/
└── integration.rs        ← NEW: 15 integration tests + 7 unit tests

plugins/
├── plugin-*.json         ← UPDATED: tool_schema + require_signature fields
└── plugin-*/src/main.rs  ← UPDATED: standardised args payload
```
