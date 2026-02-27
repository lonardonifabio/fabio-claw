# 🦀 Fabio-Claw

**Fabio-Claw** is an edge-first, embedded LLM runtime built in Rust — designed for serious edge AI deployments on Raspberry Pi and other Linux ARM boards.

> Run private, local AI inference without cloud dependency. OpenAI-compatible API. Near-zero boot time. Production security architecture.

---

## Why Fabio-Claw?

| Engine | Language | RAM | Startup | Cost Target |
|---|---|---|---|---|
| OpenClaw | TypeScript | >1GB | >500s | Mac Mini $599 |
| NanoBot | Python | >100MB | >30s | Linux SBC ~$50 |
| PicoClaw | Go | <10MB | <1s | Any Linux board ~$10 |
| **Fabio-Claw** | **Rust** | **Optimized for 8GB** | **Near-instant (preloaded)** | **Raspberry Pi / Edge** |

Fabio-Claw targets environments where:
- Cloud dependency is unacceptable (air-gapped, privacy-first, offline)
- Security and auditability matter
- Deterministic behavior is required
- You want OpenAI SDK compatibility without the cloud bill

---

## Architecture

```
HTTP API (axum)
     │
     ▼
Request Queue (mpsc, bounded=32)   ← Backpressure protection
     │
     ▼
ReAct Agent Loop (optional)        ← Thought → Action → Observation → Answer
     │
     ▼
LLM Worker (single OS thread)      ← No concurrent model access, deterministic
     │
     ▼
llama.cpp inference                ← Local GGUF model, fully offline
     │
     ▼
SQLite Memory + Audit              ← Conversation history, RAG chunks, tasks
     │
     ▼
Plugin Sandbox (child process)     ← JSON over STDIN/STDOUT, 10s timeout, sig-verified
```

---

## Features

### ✅ Implemented
- **OpenAI-compatible API** — `POST /v1/chat/completions`, `GET /v1/models`, `GET /v1/tools`
- **Single-threaded LLM actor** — deterministic, no async mutex around model
- **Bounded request queue** — backpressure protection, 60s inference timeout
- **SQLite memory layer** — conversation persistence, RAG chunks, audit logging
- **Device cryptographic identity** — Ed25519 keypair, generated on first boot
- **Sandboxed plugin system** — process isolation, signature verification, hard timeout
- **Health endpoints** — `/health`, `/health/ready`
- **Graceful shutdown** — SIGTERM + Ctrl-C handled
- **Mock inference mode** — runs without a model file for development/testing
- **Chat UI** — browser-based chat interface served via HTTP
- **ReAct agent loop** — Thought/Action/Observation cycle with plugin tool calling
- **OpenAI-style tool definitions** — plugin manifests expose `tool_schema` at `GET /v1/tools`
- **Telegram bot integration** — long-polling, zero-CPU idle, allowlist support
- **Persistent task scheduler** — cron-backed, survives reboots, REST API
- **Local RAG in SQLite** — cosine-similarity search over embedded document chunks
- **Real token accounting** — BPE estimator + actual llama.cpp token counts

---

## Project Structure

```
fabio-claw/
├── Cargo.toml
├── README.md
├── chat.html            # Browser chat UI
├── src/
│   ├── main.rs              # Entry point, config, server bootstrap
│   ├── lib.rs               # Library entry point (for integration tests)
│   ├── errors.rs            # Unified error types with HTTP mapping
│   ├── agent/
│   │   └── mod.rs           # ReAct agent loop (AGENT_MODE=true)
│   ├── api/
│   │   ├── mod.rs           # Router, AppState, /v1/tools endpoint
│   │   ├── chat.rs          # POST /v1/chat/completions
│   │   ├── health.rs        # GET /health, /health/ready
│   │   ├── models.rs        # GET /v1/models
│   │   └── scheduler.rs     # GET|POST /v1/tasks, DELETE /v1/tasks/:name
│   ├── llm/
│   │   └── mod.rs           # LLM actor, BPE token estimator, InferResult
│   ├── memory/
│   │   └── mod.rs           # SQLite: conversations, RAG chunks, scheduled tasks
│   ├── plugins/
│   │   └── mod.rs           # PluginRegistry, PluginRunner (async, sig-verified)
│   ├── scheduler/
│   │   └── mod.rs           # Persistent cron scheduler, next_run_time
│   ├── security/
│   │   └── mod.rs           # Ed25519 device identity, plugin verification
│   └── telegram/
│       └── mod.rs           # Telegram long-polling bot
└── tests/
    └── integration.rs       # 15 integration tests + 7 unit tests

plugins/
├── Cargo.toml               # Workspace
├── plugin-calculator.json   # Manifest (runtime reads this)
├── plugin-datetime.json
├── plugin-weather.json
├── plugin-file-reader.json
├── plugin-calculator/
│   ├── Cargo.toml
│   └── src/main.rs
├── plugin-datetime/
│   ├── Cargo.toml
│   └── src/main.rs
├── plugin-weather/
│   ├── Cargo.toml
│   └── src/main.rs
└── plugin-file-reader/
    ├── Cargo.toml
    └── src/main.rs
```

---

## 🍓 Complete Installation Guide — Raspberry Pi 4 from Scratch

### Hardware Requirements
- Raspberry Pi minimum** (8GB recommended for Mistral 7B)
- **32GB+ SD card** or USB SSD
- Raspberry Pi OS **64-bit** (Bookworm recommended)
- Internet connection for initial setup only

---

### Phase 1 — System Preparation

**Update the system:**
```bash
sudo apt update && sudo apt upgrade -y
```

**Install system dependencies:**
```bash
sudo apt install -y \
    build-essential \
    git \
    cmake \
    libssl-dev \
    pkg-config \
    sqlite3 \
    libsqlite3-dev \
    libclang-dev \
    clang \
    curl \
    wget
```

**Install Rust:**
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Choose option 1 (default install)
source "$HOME/.cargo/env"

# Verify
rustc --version    # should print rustc 1.75+
cargo --version
```

---

### Phase 2 — Clone the Repository

```bash
cd ~
git clone https://github.com/lonardonifabio/fabio-claw.git
cd fabio-claw
```

Verify the source structure:
```bash
find src -name "*.rs" | sort
```

Expected output:
```
src/agent/mod.rs
src/api/chat.rs
src/api/health.rs
src/api/mod.rs
src/api/models.rs
src/api/scheduler.rs
src/errors.rs
src/lib.rs
src/llm/mod.rs
src/main.rs
src/memory/mod.rs
src/plugins/mod.rs
src/scheduler/mod.rs
src/security/mod.rs
src/telegram/mod.rs
```

---

### Phase 3 — Create Runtime Directories

```bash
sudo mkdir -p /opt/fabio-claw/models
sudo mkdir -p /opt/fabio-claw/plugins
sudo mkdir -p /var/lib/fabio-claw
sudo chown -R $USER:$USER /opt/fabio-claw /var/lib/fabio-claw
```

---

### Phase 4 — Download a GGUF Model

Choose a model based on your available RAM:

| Model | File Size | RAM Required | Quality |
|---|---|---|---|
| TinyLlama 1.1B Q4 | ~700MB | ~1GB | Fast, basic |
| Phi-2 2.7B Q4 | ~1.6GB | ~2GB | Good balance |
| Mistral 7B Q4 | ~4GB | ~5GB | High quality ✅ recommended |
| Llama 3.2 8B Q4 | ~4.7GB | ~6GB | Best (8GB Pi only) |

⚠️ Due to the Raspberry Pi's limited computational power, please do not expect excellent performance.

**TinyLlama ():**
```bash
wget -O /opt/fabio-claw/models/model.gguf \
  https://huggingface.co/TheBloke/TinyLlama-1.1B-Chat-v1.0-GGUF/resolve/main/tinyllama-1.1b-chat-v1.0.Q4_K_M.gguf
```

**Phi-2 2.7B Q4:**
```bash
wget -O /opt/fabio-claw/models/phi-2.Q4_K_M.gguf \
https://huggingface.co/TheBloke/Phi-2-GGUF/resolve/main/phi-2.Q4_K_M.gguf
```

**Mistral 7B ():**
```bash
wget -O /opt/fabio-claw/models/model.gguf \
  https://huggingface.co/TheBloke/Mistral-7B-Instruct-v0.2-GGUF/resolve/main/mistral-7b-instruct-v0.2.Q4_K_M.gguf
```
**Gemma 2b:**
```bash
wget -O /opt/fabio-claw/models/gemma-2-2b-it.q4_k_m.gguf \
  https://huggingface.co/unsloth/gemma-2-it-GGUF/resolve/main/gemma-2-2b-it.q4_k_m.gguf
```

Verify the download:
```bash
ls -lh /opt/fabio-claw/models/model.gguf
# TinyLlama: ~636MB  |  Mistral 7B: ~4.1GB
```

---

### Phase 5 — Build the Project

```bash
cd ~/fabio-claw
cargo build --release
```

> ⚠️ **First build takes 20–40 minutes** on Raspberry Pi 4 — llama.cpp is compiled from source. Subsequent builds are much faster (incremental).

When complete you will see:
```
Finished `release` profile [optimized] target(s) in XX:XX
```

**Build plugins:**
```bash
cd ~/fabio-claw/plugins
cargo build --release
```

**Install the binaries system-wide:**
```bash
sudo cp ~/fabio-claw/target/release/fabio-claw /usr/local/bin/

sudo cp ~/fabio-claw/plugins/target/release/plugin-calculator  /opt/fabio-claw/plugins/
sudo cp ~/fabio-claw/plugins/target/release/plugin-datetime    /opt/fabio-claw/plugins/
sudo cp ~/fabio-claw/plugins/target/release/plugin-weather     /opt/fabio-claw/plugins/
sudo cp ~/fabio-claw/plugins/target/release/plugin-file-reader /opt/fabio-claw/plugins/

sudo cp ~/fabio-claw/plugins/plugin-calculator.json   /opt/fabio-claw/plugins/
sudo cp ~/fabio-claw/plugins/plugin-datetime.json     /opt/fabio-claw/plugins/
sudo cp ~/fabio-claw/plugins/plugin-weather.json      /opt/fabio-claw/plugins/
sudo cp ~/fabio-claw/plugins/plugin-file-reader.json  /opt/fabio-claw/plugins/
```

---

### Phase 6 — Install as a systemd Service

This makes Fabio-Claw start automatically on every boot:

```bash
sudo tee /etc/systemd/system/fabio-claw.service > /dev/null <<EOF
[Unit]
Description=Fabio-Claw Edge LLM Runtime
After=network.target

[Service]
Type=simple
User=pi
ExecStart=/usr/local/bin/fabio-claw
Restart=on-failure
RestartSec=5
StandardOutput=journal
StandardError=journal

Environment=HOST=0.0.0.0
Environment=PORT=8080
Environment=MODEL_PATH=/opt/fabio-claw/models/model.gguf
Environment=DB_PATH=/var/lib/fabio-claw/memory.db
Environment=KEY_PATH=/var/lib/fabio-claw/device.key
Environment=PLUGIN_DIR=/opt/fabio-claw/plugins
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable fabio-claw
sudo systemctl start fabio-claw
```

Check that it started correctly:
```bash
sudo systemctl status fabio-claw
```

Watch logs in real time:
```bash
journalctl -u fabio-claw -f
```

Wait until you see:
```
INFO  LLM worker ready (real inference mode)
INFO  HTTP server listening  addr=0.0.0.0:8080
```

---

### Phase 7 — Install the Chat UI

```bash
# Copy the UI file
cp ~/fabio-claw/chat.html ~/chat.html

# Install as a systemd service
sudo tee /etc/systemd/system/fabio-claw-ui.service > /dev/null <<EOF
[Unit]
Description=Fabio-Claw Chat UI
After=network.target

[Service]
Type=simple
User=pi
WorkingDirectory=/home/pi
ExecStart=python3 -m http.server 3000
Restart=on-failure

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable fabio-claw-ui
sudo systemctl start fabio-claw-ui
```

Find your Pi's IP address:
```bash
hostname -I
```

Open from any browser on the same network:
```
http://<PI-IP-ADDRESS>:3000/chat.html
```

You should see the chat interface with a green **ONLINE** status dot and the loaded model name in the header.

---

## ✅ Testing Guide

Run these tests in order after installation.

### Test 1 — Health Check
```bash
curl http://localhost:8080/health
```
Expected: `{"status":"ok","version":"0.2.0","device_id":"..."}`

---

### Test 2 — Readiness (model loaded)
```bash
curl http://localhost:8080/health/ready
```
Expected: `{"ready":true,"llm_loaded":true,"memory_ok":true}`

---

### Test 3 — List Models
```bash
curl http://localhost:8080/v1/models
```
Expected: JSON object containing the loaded model name.

---

### Test 4 — First Real AI Response
```bash
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "local",
    "messages": [
      {"role": "system", "content": "You are a helpful assistant."},
      {"role": "user", "content": "What is the capital of Italy?"}
    ],
    "max_tokens": 100
  }'
```
Expected: JSON response with `"content": "The capital of Italy is Rome..."`

---

### Test 5 — Measure Latency
```bash
time curl -s -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"local","messages":[{"role":"user","content":"Say hello."}],"max_tokens":30}' \
  | python3 -m json.tool
```
Expected latency:
- TinyLlama 1.1B: **3–10 seconds** for 30 tokens
- Mistral 7B: **60–120 seconds** for 100 tokens

---

### Test 6 — Plugin Commands
```bash
# Calculator
curl -s -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"local","messages":[{"role":"user","content":"/calc 2^10 - 1"}],"max_tokens":50}' \
  | python3 -m json.tool

# Weather
curl -s -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"local","messages":[{"role":"user","content":"/weather Milan"}],"max_tokens":100}' \
  | python3 -m json.tool

# DateTime
curl -s -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"local","messages":[{"role":"user","content":"/datetime"}],"max_tokens":50}' \
  | python3 -m json.tool
```

---

### Test 7 — Tool Definitions (for agent/tool-calling clients)
```bash
curl http://localhost:8080/v1/tools | python3 -m json.tool
```
Expected: JSON object with `"tools"` array, one entry per registered plugin, each containing `tool_schema`.

---

### Test 8 — Scheduled Tasks API
```bash
# List tasks
curl http://localhost:8080/v1/tasks

# Create a task
curl -X POST http://localhost:8080/v1/tasks \
  -H "Content-Type: application/json" \
  -d '{"name":"hourly-weather","cron_expr":"0 0 * * * *","command":"weather","args":"Milan","enabled":true}'

# Delete a task
curl -X DELETE http://localhost:8080/v1/tasks/hourly-weather
```

---

### Test 9 — Conversation Persistence
```bash
sqlite3 /var/lib/fabio-claw/memory.db \
  "SELECT datetime(created_at), user_msg, assistant_msg FROM conversations ORDER BY id DESC LIMIT 5;"
```
Expected: Your recent conversations saved with timestamps.

---

### Test 10 — Device Identity
```bash
# Check key file permissions (must be 600)
ls -la /var/lib/fabio-claw/device.key

# Check device ID in API
curl -s http://localhost:8080/health | python3 -m json.tool | grep device_id
```
Expected: `-rw-------` permissions and a 64-character hex device ID.

---

### Test 11 — Remote Access from Another Device
```bash
hostname -I   # find Pi IP
```
From your laptop or phone on the same WiFi, open:
```
http://<PI-IP>:3000/chat.html
```
Expected: Chat UI loads with green ONLINE dot, model name visible, chat works.

---

### Test 12 — Survive Reboot
```bash
sudo reboot
```
After ~60 seconds (model loading takes time on boot):
```bash
curl http://localhost:8080/health/ready
```
Expected: `{"ready":true,...}` — both services restart automatically.

---

## Configuration Reference

All configuration via environment variables (set in the systemd service file):

| Variable | Default | Description |
|---|---|---|
| `HOST` | `0.0.0.0` | Bind address |
| `PORT` | `8080` | HTTP port |
| `MODEL_PATH` | `/opt/fabio-claw/models/model.gguf` | Path to GGUF model file |
| `DB_PATH` | `/var/lib/fabio-claw/memory.db` | SQLite database path |
| `KEY_PATH` | `/var/lib/fabio-claw/device.key` | Ed25519 private key path |
| `PLUGIN_DIR` | `/opt/fabio-claw/plugins` | Plugin binary directory |
| `RUST_LOG` | `info` | Log level (`debug`, `info`, `warn`, `error`) |
| `AGENT_MODE` | `false` | Enable ReAct agent loop |
| `AGENT_TRACE` | `0` | Append agent step trace to replies |
| `TELEGRAM_TOKEN` | — | Enable Telegram bot (long-polling) |
| `TELEGRAM_ALLOWED_CHATS` | — | Comma-separated chat ID allowlist |

---

## API Reference

### `POST /v1/chat/completions`

OpenAI-compatible chat endpoint.

```json
{
  "model": "local",
  "messages": [
    {"role": "system", "content": "You are a helpful assistant."},
    {"role": "user",   "content": "Explain edge computing in 2 sentences."}
  ],
  "max_tokens": 512,
  "temperature": 0.7,
  "session_id": "optional-uuid-for-conversation-grouping"
}
```

### `GET /v1/models`
Returns the loaded model name in OpenAI list format.

### `GET /v1/tools`
Returns all registered plugin tool schemas in OpenAI format (for agent tool-calling clients).

### `GET /health`
Returns `status`, `version`, `timestamp`, and `device_id`.

### `GET /health/ready`
Returns `ready`, `llm_loaded`, `memory_ok`. Use for load balancer probes.

### `GET /v1/tasks`
List all scheduled tasks.

### `POST /v1/tasks`
Create or update a scheduled task.

### `DELETE /v1/tasks/:name`
Delete a scheduled task by name.

---

## Services & Ports

| Service | Port | Description |
|---|---|---|
| API (REST) | `8080` | OpenAI-compatible endpoint |
| Chat UI | `3000` | Browser chat interface |

| Path | Description |
|---|---|
| `/var/lib/fabio-claw/memory.db` | SQLite conversation database |
| `/var/lib/fabio-claw/device.key` | Ed25519 device private key (600 permissions) |
| `/opt/fabio-claw/models/model.gguf` | GGUF model file |
| `/opt/fabio-claw/plugins/` | Plugin binaries directory |

---

## Useful Management Commands

```bash
# Service status
sudo systemctl status fabio-claw
sudo systemctl status fabio-claw-ui

# Live logs
journalctl -u fabio-claw -f

# Restart after binary update
sudo cp ~/fabio-claw/target/release/fabio-claw /usr/local/bin/
sudo systemctl restart fabio-claw

# Inspect the database
sqlite3 /var/lib/fabio-claw/memory.db ".tables"
sqlite3 /var/lib/fabio-claw/memory.db "SELECT count(*) FROM conversations;"
sqlite3 /var/lib/fabio-claw/memory.db \
  "SELECT datetime(created_at), user_msg, assistant_msg FROM conversations ORDER BY id DESC LIMIT 10;"

# Stop all services
sudo systemctl stop fabio-claw fabio-claw-ui

# Disable autostart
sudo systemctl disable fabio-claw fabio-claw-ui
```

---

## Cross-Compile from Mac/Linux (faster builds)

Building on Pi is slow. Cross-compile on your laptop and deploy the binary:

```bash
# On your development machine
rustup target add aarch64-unknown-linux-gnu

# macOS
brew install aarch64-unknown-linux-gnu

# Linux
sudo apt install gcc-aarch64-linux-gnu

# Build
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
cargo build --release --target aarch64-unknown-linux-gnu

# Deploy to Pi
scp target/aarch64-unknown-linux-gnu/release/fabio-claw pi@<PI-IP>:/tmp/
ssh pi@<PI-IP> "sudo cp /tmp/fabio-claw /usr/local/bin/ && sudo systemctl restart fabio-claw"
```

---

## Development

```bash
# Run locally with mock inference (no model file needed)
MODEL_PATH=/nonexistent \
DB_PATH=/tmp/test.db \
KEY_PATH=/tmp/test.key \
RUST_LOG=debug \
cargo run

# Run tests
cargo test

# Run integration tests only
cargo test --test integration

# Lint
cargo clippy -- -D warnings

# Format
cargo fmt
```

---

## Security Model

- **No shared-memory plugins** — plugins run as isolated child processes
- **No `dlopen`** — no dynamic library loading at runtime
- **Signed plugin verification** — Ed25519 signatures checked before execution (opt-in via `require_signature`)
- **Device-bound cryptographic identity** — unique per device, `0600` file permissions
- **Hard timeouts** — 60s inference, 10s plugin execution
- **Backpressure** — bounded queue (32 requests) prevents memory exhaustion
- **WAL SQLite** — crash-safe writes

---

## Plugin System

Plugins are standalone executables communicating via JSON over STDIN/STDOUT.
Each manifest can declare a `tool_schema` for OpenAI-style agent tool-calling integration.
See [ADDING_A_PLUGIN.md](ADDING_A_PLUGIN.md) for a step-by-step guide.

---

## License

MIT License — Copyright (c) 2026 Fabio Lonardoni

---

## Author

**Fabio Lonardoni**
Edge-first AI Runtime Engine · [GitHub](https://github.com/lonardonifabio/fabio-claw)

---

> Fabio-Claw is not a wrapper. It is an operating layer for embedded AI.
