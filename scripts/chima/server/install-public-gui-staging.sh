#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

DEST_DIR="${DEST_DIR:-/opt/codex-chima-v1/public-gui}"
TEMPLATE_DIR="${TEMPLATE_DIR:-/opt/codex-chima-v1/proxy-templates}"

if [[ "${EUID:-$(id -u)}" -ne 0 ]]; then
  echo "Run as root, for example: sudo $0" >&2
  exit 1
fi

install -d -m 0755 "$DEST_DIR"
install -d -m 0755 "$DEST_DIR/assets"
install -d -m 0755 "$TEMPLATE_DIR"

install -m 0644 "$SCRIPT_DIR/public-gui/index.html" "$DEST_DIR/index.html"
install -m 0644 "$SCRIPT_DIR/public-gui/assets/app.js" "$DEST_DIR/assets/app.js"
install -m 0644 "$SCRIPT_DIR/public-gui/assets/styles.css" "$DEST_DIR/assets/styles.css"
install -m 0644 "$SCRIPT_DIR/proxy-templates/caddy.codex-chima-v1.example" "$TEMPLATE_DIR/caddy.codex-chima-v1.example"
install -m 0644 "$SCRIPT_DIR/proxy-templates/nginx.codex-chima-v1.example" "$TEMPLATE_DIR/nginx.codex-chima-v1.example"

cat <<EOF
Installed Codex CHIMA v1 public GUI staging files.

Static GUI:      $DEST_DIR
Proxy templates: $TEMPLATE_DIR

No public route was enabled.

The staged GUI expects an authenticated reverse proxy to:
  - serve $DEST_DIR
  - proxy /codex-chima-app/* to http://127.0.0.1:4222/
  - strip the Origin header before forwarding app-server WebSockets

Review templates before enabling:
  $TEMPLATE_DIR/caddy.codex-chima-v1.example
  $TEMPLATE_DIR/nginx.codex-chima-v1.example
EOF
