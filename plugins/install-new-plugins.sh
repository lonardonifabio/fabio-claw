#!/bin/bash
# install-new-plugins.sh — Build and install the 10 new Fabio-Claw plugins
# Run from ~/fabio-claw/plugins directory
#
# Usage:
#   cd ~/fabio-claw/plugins
#   bash install-new-plugins.sh

set -e

PLUGIN_DIR="/opt/fabio-claw/plugins"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "🦀 Fabio-Claw Plugin Installer — v0.3.0 plugins"
echo "================================================="

# ── Ensure output dir exists ──────────────────────────────────────────────────
sudo mkdir -p "$PLUGIN_DIR"
sudo mkdir -p /var/lib/fabio-claw/documents
sudo mkdir -p /tmp/fabio-claw/documents

# ── Build Rust plugins ────────────────────────────────────────────────────────
echo ""
echo "▶ Building Rust plugins (this takes a few minutes on Raspberry Pi)..."

cd "$SCRIPT_DIR"
cargo build --release \
    -p plugin-system-info \
    -p plugin-net-diagnostics \
    -p plugin-log-tail \
    -p plugin-scheduler \
    -p plugin-rag-local \
    -p plugin-gpio-control \
    -p plugin-updater \
    -p plugin-rag-internet

echo "✅ Rust build complete"

# ── Install Rust binaries ─────────────────────────────────────────────────────
echo ""
echo "▶ Installing Rust binaries..."

RUST_PLUGINS=(
    plugin-system-info
    plugin-net-diagnostics
    plugin-log-tail
    plugin-scheduler
    plugin-rag-local
    plugin-gpio-control
    plugin-updater
    plugin-rag-internet
)

for plugin in "${RUST_PLUGINS[@]}"; do
    sudo cp "$SCRIPT_DIR/target/release/$plugin" "$PLUGIN_DIR/"
    sudo cp "$SCRIPT_DIR/$plugin.json" "$PLUGIN_DIR/"
    echo "  ✓ $plugin"
done

# ── Install Python plugins ────────────────────────────────────────────────────
echo ""
echo "▶ Installing Python plugins..."

# Check Python dependencies
if ! python3 -c "import docx" 2>/dev/null; then
    echo "  ⚠ python-docx not found. Installing..."
    pip install python-docx --break-system-packages --quiet || \
        echo "  ✗ Failed to install python-docx. Install manually: pip install python-docx --break-system-packages"
fi

if ! python3 -c "import openpyxl" 2>/dev/null; then
    echo "  ⚠ openpyxl not found. Installing..."
    pip install openpyxl --break-system-packages --quiet || \
        echo "  ✗ Failed to install openpyxl. Install manually: pip install openpyxl --break-system-packages"
fi

sudo cp "$SCRIPT_DIR/plugin-word-builder"  "$PLUGIN_DIR/"
sudo cp "$SCRIPT_DIR/plugin-word-builder.json"  "$PLUGIN_DIR/"
sudo chmod +x "$PLUGIN_DIR/plugin-word-builder"

sudo cp "$SCRIPT_DIR/plugin-excel-builder" "$PLUGIN_DIR/"
sudo cp "$SCRIPT_DIR/plugin-excel-builder.json" "$PLUGIN_DIR/"
sudo chmod +x "$PLUGIN_DIR/plugin-excel-builder"

echo "  ✓ plugin-word-builder"
echo "  ✓ plugin-excel-builder"

# ── Restart runtime ───────────────────────────────────────────────────────────
echo ""
echo "▶ Restarting fabio-claw service..."
sudo systemctl restart fabio-claw

sleep 2

echo ""
echo "▶ Verifying plugin registration..."
journalctl -u fabio-claw --no-pager -n 30 | grep "Registered plugin" || \
    echo "  (check: journalctl -u fabio-claw | grep 'Registered plugin')"

echo ""
echo "================================================="
echo "✅ All plugins installed. Test with:"
echo ""
echo "  curl -s -X POST http://localhost:8080/v1/chat/completions \\"
echo "    -H 'Content-Type: application/json' \\"
echo "    -d '{\"model\":\"local\",\"messages\":[{\"role\":\"user\",\"content\":\"/sysinfo\"}],\"max_tokens\":200}' \\"
echo "    | python3 -m json.tool"
echo ""
echo "  /sysinfo        — system metrics"
echo "  /ping 8.8.8.8   — network ping"
echo "  /logs 30        — last 30 log lines"
echo "  /remind 30min Check the Pi  — set reminder"
echo "  /kb             — list local knowledge base"
echo "  /gpio status    — GPIO pin status"
echo "  /update-check   — check for updates"
echo "  /web-search Raspberry Pi GPIO — web search"
echo "  /make-docx meeting minutes  — create Word doc"
echo "  /make-xlsx budget           — create Excel sheet"
echo "  /help           — all commands"
