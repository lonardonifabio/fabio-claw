# 🔌 Fabio-Claw — New Plugins (v0.3.0)

Ten new plugins extending Fabio-Claw with edge-ops monitoring, IoT control, document generation, and web RAG.

---

## Quick Install

```bash
cd ~/fabio-claw/plugins
bash install-new-plugins.sh
```

---

## Plugin Reference

### 1. plugin-system-info

**Commands:** `/sysinfo` · `/uptime` · `/disk`  
**Language:** Rust  
**Purpose:** Edge operations monitoring — CPU temperature, free RAM, system uptime, disk usage.

```bash
# Full system report
/sysinfo

# Just uptime
/uptime

# Disk usage of root filesystem
/disk
```

**Output fields:**
- `cpu_temp_celsius` — Raspberry Pi thermal zone (null on x86)
- `memory.used_pct` — RAM utilisation percentage
- `uptime` / `uptime_seconds` — human-readable and raw seconds
- `load_avg.1min / 5min / 15min` — Linux load averages
- `disk_root.available` / `use_pct` — root filesystem stats

---

### 2. plugin-net-diagnostics

**Commands:** `/ping <host>` · `/dns <host>` · `/latency <host>`  
**Language:** Rust  
**Purpose:** Network reachability and latency checks without external tools.

```bash
/ping 8.8.8.8
/dns google.com
/latency github.com
```

**Output:**
- `/ping` — reachable boolean, packet loss %, RTT min/avg/max
- `/dns` — resolved IP addresses, resolution time in ms
- `/latency` — TCP connect time on ports 443/80/22

Security: host is sanitized (alphanumeric + dots/hyphens only, max 253 chars).

---

### 3. plugin-log-tail

**Commands:** `/logs [n]` · `/errors [n]`  
**Language:** Rust  
**Purpose:** Tail application logs with automatic secret sanitization.

```bash
/logs 50       # last 50 log lines
/errors        # only ERROR/WARN lines
/errors 20     # last 20 error lines
```

**Features:**
- Uses `journalctl -u fabio-claw` on systemd systems
- Falls back to `/var/log/syslog` or `/var/log/messages`
- **Sanitizes** values after `password`, `token`, `key`, `secret`, `auth` → `[REDACTED]`
- Truncates lines >300 chars
- Reports error count alongside log lines

---

### 4. plugin-scheduler

**Commands:** `/remind <time> <message>` · `/jobs`  
**Language:** Rust + SQLite  
**Purpose:** Local reminder system with persistent storage.

```bash
/remind 30min Check the oven
/remind 2h Team standup
/remind 14:30 Deployment window
/remind tomorrow Morning review
/jobs           # list all pending reminders
```

**Time formats:** `30s`, `10min`, `2h`, `1h30m`, `HH:MM`, `tomorrow`  
**Storage:** SQLite at `/var/lib/fabio-claw/reminders.db` (or `/tmp/fabio-reminders.db`)

---

### 5. plugin-rag-local

**Commands:** `/kb [query]` · `/search-doc <query>`  
**Language:** Rust + SQLite FTS5  
**Purpose:** Index and search local documents using full-text search.

```bash
# Index documents
/kb index /home/pi/manuals

# Search the knowledge base
/kb GPIO setup raspberry pi
/search-doc maintenance procedure

# List indexed sources
/kb
```

**Allowed directories:** `/home/pi/documents`, `/home/pi/manuals`, `/home/pi/data`, `/opt/fabio-claw/kb`, `/tmp/fabio-claw/kb`  
**Supported file types:** `.txt`, `.md`, `.rst`, `.csv`, `.log`, `.json`, `.yaml`, `.toml`  
**Search method:** SQLite FTS5 with BM25 ranking, falls back to LIKE search

---

### 6. plugin-gpio-control

**Commands:** `/gpio on <pin>` · `/gpio off <pin>` · `/gpio read <pin>` · `/gpio status`  
**Language:** Rust  
**Purpose:** Raspberry Pi GPIO control via Linux sysfs — no external libraries required.

```bash
/gpio on 17       # Set BCM pin 17 HIGH
/gpio off 17      # Set BCM pin 17 LOW
/gpio read 17     # Read current state of pin 17
/gpio status      # List all currently exported pins
```

**Safe pins:** BCM 2-27 (standard 40-pin header, excludes reserved pins)  
**Interface:** `/sys/class/gpio` sysfs — works on all Pi models  
**Requires:** User must be in the `gpio` group: `sudo adduser pi gpio`

---

### 7. plugin-updater

**Commands:** `/update-check` · `/update-plan`  
**Language:** Rust  
**Purpose:** Read-only update checker — never executes destructive commands.

```bash
/update-check          # Check apt + fabio-claw version
/update-check system   # Check system packages only
/update-check fabio    # Check runtime only
/update-plan           # Generate step-by-step update instructions
```

**Safety guarantees:**
- Uses `apt-get -s` (simulate mode, no changes)
- Uses `git fetch --dry-run` (no checkout)
- Returns a plan document — the user must execute manually
- All responses include `"safe_mode": true`

---

### 8. plugin-rag-internet

**Commands:** `/web-search <query>` · `/web-rag <query>`  
**Language:** Rust + ureq  
**Purpose:** Web search with source citations using DuckDuckGo (no API key needed).

```bash
/web-search Raspberry Pi 5 GPIO pinout
/web-rag how to install fabio-claw edge AI
```

**`/web-search`** returns: title, snippet, URL for each result  
**`/web-rag`** returns: synthesized summary with numbered citations `[1]`, `[2]`...  
**Source:** DuckDuckGo Instant Answer API (free, no registration, GDPR-friendly)

---

### 9. plugin-word-builder

**Commands:** `/make-docx <description>` · `/doc-template <name>`  
**Language:** Python 3 + python-docx  
**Dependency:** `pip install python-docx --break-system-packages`

```bash
# Free-form document
/make-docx meeting minutes for project Titan 2024-02-26

# From template
/doc-template meeting-minutes
/doc-template report
/doc-template sop
/doc-template letter

# List templates
/doc-template
```

**Available templates:** `meeting-minutes`, `report`, `letter`, `sop`  
**Output:** `.docx` file at `/var/lib/fabio-claw/documents/` with formatted headings, date, footer  
**Startup time:** ~200ms (Python) vs ~5ms (Rust)

---

### 10. plugin-excel-builder

**Commands:** `/make-xlsx <description>` · `/sheet-template <name>`  
**Language:** Python 3 + openpyxl  
**Dependency:** `pip install openpyxl --break-system-packages`

```bash
# From template
/sheet-template budget           # Monthly budget tracker with formulas
/sheet-template inventory        # Parts inventory with LOW STOCK alert formulas
/sheet-template sensor-log       # IoT sensor data log with ALERT status
/sheet-template kpi              # KPI dashboard with target vs actual
/sheet-template task-tracker     # Project task tracker

# Free-form
/make-xlsx monthly expenses Italy trip
/make-xlsx sales data by region

# List templates
/sheet-template
```

**Features:**
- Professional formatting: bold headers, color fill (dark blue `#1F4E79`), Arial font
- Working Excel formulas (variance, SUM, IF, COUNTIF, AVERAGE)
- Status formulas: `⚠ LOW STOCK` / `✓ OK`, `⚠ ALERT` / `✓ NORMAL`
- Output at `/var/lib/fabio-claw/documents/`

---

## File Layout After Install

```
plugins/
├── Cargo.toml                        ← UPDATED workspace
├── install-new-plugins.sh            ← installer
├── NEW_PLUGINS.md                    ← this document
│
├── plugin-system-info/               ← Rust
├── plugin-system-info.json
├── plugin-net-diagnostics/           ← Rust
├── plugin-net-diagnostics.json
├── plugin-log-tail/                  ← Rust
├── plugin-log-tail.json
├── plugin-scheduler/                 ← Rust + SQLite
├── plugin-scheduler.json
├── plugin-rag-local/                 ← Rust + SQLite FTS5
├── plugin-rag-local.json
├── plugin-gpio-control/              ← Rust
├── plugin-gpio-control.json
├── plugin-updater/                   ← Rust
├── plugin-updater.json
├── plugin-rag-internet/              ← Rust + ureq
├── plugin-rag-internet.json
├── plugin-word-builder               ← Python 3 script (no Cargo.toml)
├── plugin-word-builder.json
├── plugin-excel-builder              ← Python 3 script (no Cargo.toml)
└── plugin-excel-builder.json
```

---

## Testing All New Plugins

```bash
# After install, test each plugin via the API:

BASE="http://localhost:8080/v1/chat/completions"
H='-H "Content-Type: application/json"'

curl -s -X POST $BASE -H "Content-Type: application/json" \
  -d '{"model":"local","messages":[{"role":"user","content":"/sysinfo"}],"max_tokens":300}' \
  | python3 -m json.tool | grep content

curl -s -X POST $BASE -H "Content-Type: application/json" \
  -d '{"model":"local","messages":[{"role":"user","content":"/ping 8.8.8.8"}],"max_tokens":200}' \
  | python3 -m json.tool | grep content

curl -s -X POST $BASE -H "Content-Type: application/json" \
  -d '{"model":"local","messages":[{"role":"user","content":"/logs 10"}],"max_tokens":300}' \
  | python3 -m json.tool | grep content

curl -s -X POST $BASE -H "Content-Type: application/json" \
  -d '{"model":"local","messages":[{"role":"user","content":"/remind 10min Test reminder"}],"max_tokens":100}' \
  | python3 -m json.tool | grep content

curl -s -X POST $BASE -H "Content-Type: application/json" \
  -d '{"model":"local","messages":[{"role":"user","content":"/kb"}],"max_tokens":200}' \
  | python3 -m json.tool | grep content

curl -s -X POST $BASE -H "Content-Type: application/json" \
  -d '{"model":"local","messages":[{"role":"user","content":"/gpio status"}],"max_tokens":200}' \
  | python3 -m json.tool | grep content

curl -s -X POST $BASE -H "Content-Type: application/json" \
  -d '{"model":"local","messages":[{"role":"user","content":"/update-check"}],"max_tokens":300}' \
  | python3 -m json.tool | grep content

curl -s -X POST $BASE -H "Content-Type: application/json" \
  -d '{"model":"local","messages":[{"role":"user","content":"/web-search Raspberry Pi GPIO"}],"max_tokens":300}' \
  | python3 -m json.tool | grep content

curl -s -X POST $BASE -H "Content-Type: application/json" \
  -d '{"model":"local","messages":[{"role":"user","content":"/make-docx project status report"}],"max_tokens":200}' \
  | python3 -m json.tool | grep content

curl -s -X POST $BASE -H "Content-Type: application/json" \
  -d '{"model":"local","messages":[{"role":"user","content":"/sheet-template budget"}],"max_tokens":200}' \
  | python3 -m json.tool | grep content
```

---

## /help Output After Install

```
/calc               Math expression evaluator
/date               Current date, time and timezone
/datetime           Current date, time and timezone
/disk               System info: CPU temp, RAM, uptime, disk
/dns                Network diagnostics: ping, DNS, latency
/errors             Tail application logs with filtering
/file               Read local files safely
/forecast           Live weather + 3-day forecast
/gpio               Raspberry Pi GPIO control (IoT)
/jobs               Local reminders and job listing
/kb                 Local document search (RAG)
/latency            Network diagnostics: ping, DNS, latency
/logs               Tail application logs with filtering
/make-docx          Generate Word .docx files from prompts
/make-xlsx          Generate Excel .xlsx files with sheets
/math               Math expression evaluator
/meteo              Live weather + 3-day forecast
/now                Current date, time and timezone
/ping               Network diagnostics: ping, DNS, latency
/remind             Local reminders and job listing
/search-doc         Local document search (RAG)
/sheet-template     Generate Excel .xlsx files with sheets
/sysinfo            System info: CPU temp, RAM, uptime, disk
/time               Current date, time and timezone
/update-check       Safe update check and plan (read-only)
/update-plan        Safe update check and plan (read-only)
/uptime             System info: CPU temp, RAM, uptime, disk
/web-rag            Web search + RAG with source citations
/web-search         Web search + RAG with source citations
/weather            Live weather + 3-day forecast
/doc-template       Generate Word .docx files from prompts
```
