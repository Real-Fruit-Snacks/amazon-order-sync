#!/usr/bin/env bash
set -euo pipefail

APP_NAME="amazon-order-sync"
INSTALL_DIR="$HOME/.local/share/$APP_NAME"
SYSTEMD_USER_DIR="$HOME/.config/systemd/user"
UNIT_NAME="$APP_NAME.service"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/$APP_NAME"
CONFIG_FILE="$CONFIG_DIR/config.json"
PORT=27483
HEALTH_URL="http://127.0.0.1:$PORT/api/health"
DASHBOARD_URL="http://127.0.0.1:$PORT/"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

uninstall() {
    echo "Uninstalling $APP_NAME..."
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user disable --now "$UNIT_NAME" >/dev/null 2>&1 || true
    fi
    rm -f "$SYSTEMD_USER_DIR/$UNIT_NAME"
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user daemon-reload >/dev/null 2>&1 || true
    fi
    # Remove the app dir contents but preserve browser-profile/ (it holds the
    # headless auto-sync Amazon session; v1.5.0). Notes and config untouched.
    if [[ -d "$INSTALL_DIR" ]]; then
        find "$INSTALL_DIR" -mindepth 1 -maxdepth 1 ! -name browser-profile -exec rm -rf {} +
        rmdir "$INSTALL_DIR" 2>/dev/null || true
    fi
    echo "Removed $INSTALL_DIR (except any browser-profile login) and the systemd user unit."
    echo "Your notes folder, $CONFIG_FILE, and the browser profile were left untouched."
}

if [[ "${1:-}" == "--uninstall" ]]; then
    uninstall
    exit 0
fi

# --- Verify Node.js >= 18 ---------------------------------------------------
if ! command -v node >/dev/null 2>&1; then
    echo "Error: node is not installed." >&2
    echo "Install it with: sudo pacman -S nodejs" >&2
    exit 1
fi
NODE_MAJOR="$(node -e 'process.stdout.write(String(process.versions.node.split(".")[0]))')"
if [[ "$NODE_MAJOR" -lt 18 ]]; then
    echo "Error: Node.js >= 18 is required (found $(node --version))." >&2
    echo "Upgrade with: sudo pacman -S nodejs" >&2
    exit 1
fi

# --- Install files ----------------------------------------------------------
echo "Installing $APP_NAME..."
mkdir -p "$INSTALL_DIR"
rm -rf "$INSTALL_DIR/server"
cp -r "$SCRIPT_DIR/server" "$INSTALL_DIR/server"

# v1.5.0: also install the companion browser extension next to the server so
# headless auto-sync can load it. Idempotent (replaces any previous copy).
rm -rf "$INSTALL_DIR/browser-extension"
cp -r "$SCRIPT_DIR/browser-extension" "$INSTALL_DIR/browser-extension"

mkdir -p "$SYSTEMD_USER_DIR"
cp "$SCRIPT_DIR/$UNIT_NAME" "$SYSTEMD_USER_DIR/$UNIT_NAME"

# v1.7.2: restart the service when it is ALREADY RUNNING so an upgrade picks
# up the freshly copied files immediately. try-restart is a no-op when the
# service isn't running (e.g. first install — `enable --now` below handles
# that case), so this is always safe.
# v1.8.3: guarded (minimal/container installs may lack systemd) and
# daemon-reload runs BEFORE try-restart so a restart picks up the freshly
# copied unit, not the cached old one.
if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload 2>/dev/null || true
    systemctl --user try-restart "$UNIT_NAME" 2>/dev/null || true
fi

# --- Default config (0600) --------------------------------------------------
if [[ ! -f "$CONFIG_FILE" ]]; then
    mkdir -p "$CONFIG_DIR"
    cat > "$CONFIG_FILE" <<'EOF'
{
  "notesPath": "~/AmazonOrders",
  "geminiApiKey": "",
  "port": 27483
}
EOF
    chmod 0600 "$CONFIG_FILE"
    echo "Created default config at $CONFIG_FILE"
fi

NOTES_PATH="$(node -e '
    const fs = require("fs"), os = require("os");
    try {
        const c = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
        const p = c.notesPath || "~/AmazonOrders";
        console.log(p.replace(/^~(?=$|[\/\\])/, os.homedir()));
    } catch { console.log(os.homedir() + "/AmazonOrders"); }
' "$CONFIG_FILE")"

# The port from the config (the health check below must use the same one).
PORT="$(node -e '
    try {
        const p = Number(JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).port);
        console.log(Number.isInteger(p) && p > 0 ? p : 27483);
    } catch { console.log(27483); }
' "$CONFIG_FILE")"
HEALTH_URL="http://127.0.0.1:$PORT/api/health"
DASHBOARD_URL="http://127.0.0.1:$PORT/"

# --- Enable and start -------------------------------------------------------
# v1.8.3: guarded — without systemd there is no service to enable; the
# health check below then simply reports the manual-start state.
if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload
    systemctl --user enable --now "$UNIT_NAME"
else
    echo "Note: systemctl not found — skipping service setup; start the server manually." >&2
fi

# --- Verify -----------------------------------------------------------------
STATUS="failed to respond"
for _ in $(seq 1 10); do
    if curl -sf --max-time 1 "$HEALTH_URL" >/dev/null 2>&1; then
        STATUS="running"
        break
    fi
    sleep 0.5
done

echo ""
echo "Server status: $STATUS"
echo "Dashboard:     $DASHBOARD_URL"
echo "Notes path:    $NOTES_PATH"
echo "Config:        $CONFIG_FILE"
echo "Extension:     load $INSTALL_DIR/browser-extension unpacked (chrome://extensions, Developer mode)"

if [[ "$STATUS" != "running" ]]; then
    echo "Warning: the server did not answer at $HEALTH_URL within 5 seconds." >&2
    echo "Check logs with: journalctl --user -u $UNIT_NAME -e" >&2
    exit 1
fi
