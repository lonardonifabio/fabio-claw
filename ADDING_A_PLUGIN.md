# 🔌 Adding a New Plugin to Fabio-Claw

No changes to the core runtime are required. You only need two things:
a binary and a JSON manifest.

---

## How it works

At startup, Fabio-Claw scans `/opt/fabio-claw/plugins/` for `*.json` files.
Each manifest declares which slash-commands the plugin handles.
The routing table is built automatically — `chat.rs` never needs to change.

```
/opt/fabio-claw/plugins/
├── my-plugin          ← compiled binary
└── my-plugin.json     ← manifest (controls routing)
```

When a user types `/mycommand some args`, Fabio-Claw:
1. Finds the manifest that lists `"mycommand"` in its `commands` array
2. Launches the binary as a child process
3. Sends a JSON request over STDIN
4. Reads the JSON response from STDOUT
5. Formats and returns the result to the user

---

## Step 1 — Write the plugin

A plugin is any executable that reads JSON from STDIN and writes JSON to STDOUT.
It can be written in any language. Here is a minimal Rust example:

```
plugins/
└── plugin-myname/
    ├── Cargo.toml
    └── src/
        └── main.rs
```

**`plugins/plugin-myname/Cargo.toml`:**
```toml
[package]
name = "plugin-myname"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "plugin-myname"
path = "src/main.rs"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

**`plugins/plugin-myname/src/main.rs`:**
```rust
use std::io::{self, Read};

fn main() {
    // 1. Read the full request from STDIN
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap_or(0);

    // 2. Parse it
    let req: serde_json::Value = serde_json::from_str(&input)
        .unwrap_or(serde_json::json!({}));

    let action = req["action"].as_str().unwrap_or("");
    let args   = req["payload"]["args"].as_str().unwrap_or("");

    // 3. Do the work
    let result = match action {
        "run" => serde_json::json!({
            "message": format!("Hello from my-plugin! You said: {}", args)
        }),
        _ => serde_json::json!(null),
    };

    // 4. Write response to STDOUT — always this exact shape
    println!("{}", serde_json::json!({
        "success": true,
        "result":  result,
        "error":   null
    }));
}
```

The response shape is always:
```json
{ "success": true,  "result": { ... }, "error": null    }
{ "success": false, "result": null,    "error": "reason" }
```

---

## Step 2 — Create the manifest JSON

Create `plugins/plugin-myname.json` next to the binary:

```json
{
  "name":             "plugin-myname",
  "version":          "0.1.0",
  "description":      "One-line description shown in /help",
  "commands":         ["mycommand", "myalias"],
  "default_action":   "run",
  "payload_from_args": true
}
```

| Field | Description |
|---|---|
| `name` | Must match the binary filename exactly |
| `commands` | Slash-commands that trigger this plugin (all lowercase) |
| `default_action` | The `action` string sent in the request payload |
| `payload_from_args` | If `true`, the text after the command is forwarded as `{"args": "..."}` |

---

## Step 3 — Add to the workspace

Add your plugin to `plugins/Cargo.toml`:

```toml
[workspace]
members = [
    "plugin-datetime",
    "plugin-calculator",
    "plugin-weather",
    "plugin-file-reader",
    "plugin-myname",      # ← add this line
]
resolver = "2"
```

---

## Step 4 — Build and install on the Pi

```bash
# Build only your new plugin (fast — incremental)
cd ~/fabio-claw/plugins
cargo build --release -p plugin-myname

# Install binary and manifest
sudo cp target/release/plugin-myname    /opt/fabio-claw/plugins/
sudo cp plugin-myname.json              /opt/fabio-claw/plugins/

# Restart the runtime to pick up the new manifest
sudo systemctl restart fabio-claw

# Verify registration in logs
journalctl -u fabio-claw | grep "Registered plugin"
```

You should see:
```
INFO  Registered plugin  plugin=plugin-myname  commands=["mycommand","myalias"]
```

---

## Step 5 — Test it

```bash
curl -s -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"local","messages":[{"role":"user","content":"/mycommand hello world"}],"max_tokens":50}' \
  | python3 -m json.tool
```

Or type `/mycommand hello world` directly in the chat UI.

Type `/help` to confirm it appears in the command list.

---

## Test a plugin directly (without the runtime)

You can test any plugin in isolation from the terminal:

```bash
echo '{"action":"run","payload":{"args":"hello world"}}' \
  | /opt/fabio-claw/plugins/plugin-myname
```

Expected output:
```json
{"success":true,"result":{"message":"Hello from my-plugin! You said: hello world"},"error":null}
```

---

## Optional: Rich response formatting

By default, unknown plugins display their JSON result as pretty-printed text.
To add custom formatting, add one entry to the `format_result()` function
in `src/api/chat.rs`:

```rust
"plugin-myname" => format!(
    "🎯 **My Plugin**\n{}",
    result["message"].as_str().unwrap_or("—")
),
```

This is the **only** time you ever need to touch `chat.rs` — and only
if you want prettier output beyond the default JSON display.

---

## Non-Rust plugins

Any language works as long as it reads STDIN and writes STDOUT.

**Python example (`plugin-myname`):**
```python
#!/usr/bin/env python3
import sys, json

req = json.loads(sys.stdin.read())
args = req.get("payload", {}).get("args", "")

print(json.dumps({
    "success": True,
    "result": {"message": f"Hello from Python plugin! Args: {args}"},
    "error": None
}))
```

```bash
chmod +x plugin-myname
sudo cp plugin-myname /opt/fabio-claw/plugins/
sudo cp plugin-myname.json /opt/fabio-claw/plugins/
sudo systemctl restart fabio-claw
```

> Python plugins are slower to start (~200ms vs ~5ms for Rust) but perfectly
> functional for non-latency-sensitive tasks.

---

## Summary checklist

```
□ Write the binary (any language)
□ Create plugin-myname.json manifest
□ Add to plugins/Cargo.toml workspace (if Rust)
□ cargo build --release -p plugin-myname
□ sudo cp binary + json → /opt/fabio-claw/plugins/
□ sudo systemctl restart fabio-claw
□ journalctl -u fabio-claw | grep "Registered"
□ Test with /mycommand or curl
```

No changes to `chat.rs`, `main.rs`, or any other core file required.
