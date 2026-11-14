#!/usr/bin/env bash
set -euo pipefail

# Amazon Order Sync - Linux installer / upgrader.
#
# Run from the folder the release tarball was extracted into (the one holding
# the amazon-order-sync program and browser-extension/), or from a source
# checkout with Rust installed (it builds the program first). It installs both
# into ~/.local/share/amazon-order-sync, writes a default config on first run,
# and runs the server as a systemd user service. Rerun to upgrade;
# --uninstall removes it (notes, config and the browser login are kept).

APP_NAME="amazon-order-sync"
# The systemd unit runs %h/.local/share/amazon-order-sync/amazon-order-sync.
INSTALL_DIR="$HOME/.local/share/$APP_NAME"
SYSTEMD_USER_DIR="$HOME/.config/systemd/user"
UNIT_NAME="$APP_NAME.service"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/$APP_NAME"
CONFIG_FILE="$CONFIG_DIR/config.json"

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
    # Remove the app dir contents but keep browser-profile/ (it holds the
    # headless auto-sync Amazon session). Notes and config are untouched.
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

# --- The program --------------------------------------------------------------
BIN="$SCRIPT_DIR/$APP_NAME"
if [[ ! -x "$BIN" ]]; then
    if [[ -f "$SCRIPT_DIR/Cargo.toml" ]] && command -v cargo >/dev/null 2>&1; then
        echo "Building $APP_NAME from source (cargo build --release)..."
        (cd "$SCRIPT_DIR" && cargo build --release --locked)
        BIN="$SCRIPT_DIR/target/release/$APP_NAME"
    else
        echo "Error: $BIN not found." >&2
        echo "Run this from the folder the release tarball was extracted into," >&2
        echo "or from a source checkout with Rust installed (https://rustup.rs)." >&2
        exit 1
    fi
fi

# --- Install files ----------------------------------------------------------
echo "Installing $APP_NAME..."
mkdir -p "$INSTALL_DIR"
# Copy beside the running binary, then rename over it: a running server keeps
# its old inode, and the restart below picks up the new one.
cp "$BIN" "$INSTALL_DIR/.$APP_NAME.new"
chmod 0755 "$INSTALL_DIR/.$APP_NAME.new"
mv -f "$INSTALL_DIR/.$APP_NAME.new" "$INSTALL_DIR/$APP_NAME"
# 2.3.0 and older ran server/server.js with Node.js; it is no longer used.
rm -rf "$INSTALL_DIR/server"

# The companion browser extension, next to the program, so headless
# auto-sync can load it. Replaces any previous copy.
rm -rf "$INSTALL_DIR/browser-extension"
cp -r "$SCRIPT_DIR/browser-extension" "$INSTALL_DIR/browser-extension"

mkdir -p "$SYSTEMD_USER_DIR"
cp "$SCRIPT_DIR/$UNIT_NAME" "$SYSTEMD_USER_DIR/$UNIT_NAME"

# Restart a service that is ALREADY RUNNING so an upgrade takes effect now.
# try-restart does nothing when it isn't running (enable --now below starts
# it); daemon-reload first so the restart uses the freshly copied unit.
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

# The notes folder and port the server will use, read by the program itself.
INFO="$(AMAZON_SYNC_CONFIG="$CONFIG_FILE" "$INSTALL_DIR/$APP_NAME" config)"
NOTES_PATH="$(printf '%s\n' "$INFO" | sed -n 's/^notesPath=//p')"
PORT="$(printf '%s\n' "$INFO" | sed -n 's/^port=//p')"
HEALTH_URL="http://127.0.0.1:${PORT:-27483}/api/health"
DASHBOARD_URL="http://127.0.0.1:${PORT:-27483}/"

# --- Enable and start -------------------------------------------------------
# Without systemd there is no service to enable; the health check below then
# reports the manual-start state.
if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload
    systemctl --user enable --now "$UNIT_NAME"
else
    echo "Note: systemctl not found — skipping service setup; start it with: $INSTALL_DIR/$APP_NAME serve" >&2
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
