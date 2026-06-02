#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../../.." && pwd)"

BINARY_PATH="${BINARY_PATH:-$REPO_ROOT/codex-rs/target-chima-linux/release/codex-chima-v1}"
INSTALL_DIR="${INSTALL_DIR:-/opt/codex-chima-v1}"
CODEX_HOME_DIR="${CODEX_HOME_DIR:-/var/lib/codex-chima-v1}"
SERVICE_USER="${SERVICE_USER:-codex-chima}"
LISTEN_URL="${LISTEN_URL:-ws://127.0.0.1:4222}"
ENV_FILE="${ENV_FILE:-/etc/codex-chima-v1.env}"
SERVICE_FILE="${SERVICE_FILE:-/etc/systemd/system/codex-chima-v1.service}"

if [[ "${EUID:-$(id -u)}" -ne 0 ]]; then
  echo "Run as root, for example: sudo $0" >&2
  exit 1
fi

if [[ ! -f "$BINARY_PATH" ]]; then
  echo "Binary not found: $BINARY_PATH" >&2
  echo "Build first: ./scripts/chima/build-codex-chima-v1-linux.sh" >&2
  exit 1
fi

if ! id -u "$SERVICE_USER" >/dev/null 2>&1; then
  useradd --system --create-home --home-dir "$CODEX_HOME_DIR" --shell /usr/sbin/nologin "$SERVICE_USER"
fi

install -d -m 0755 "$INSTALL_DIR"
install -m 0755 "$BINARY_PATH" "$INSTALL_DIR/codex-chima-v1"
ln -sfn "$INSTALL_DIR/codex-chima-v1" /usr/local/bin/codex-chima-v1

install -d -m 0750 -o "$SERVICE_USER" -g "$SERVICE_USER" "$CODEX_HOME_DIR"
chown -R "$SERVICE_USER:$SERVICE_USER" "$CODEX_HOME_DIR"

cat > "$ENV_FILE" <<EOF
CODEX_HOME=$CODEX_HOME_DIR
CODEX_CHIMA_LISTEN_URL=$LISTEN_URL
EOF
chmod 0644 "$ENV_FILE"

cat > "$SERVICE_FILE" <<EOF
[Unit]
Description=Codex CHIMA v1 app-server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$SERVICE_USER
Group=$SERVICE_USER
EnvironmentFile=$ENV_FILE
WorkingDirectory=$CODEX_HOME_DIR
ExecStart=$INSTALL_DIR/codex-chima-v1 app-server --listen \${CODEX_CHIMA_LISTEN_URL}
Restart=on-failure
RestartSec=5
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full
ProtectHome=true
ReadWritePaths=$CODEX_HOME_DIR
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload

cat <<EOF
Installed Codex CHIMA v1 server runtime.

Binary:       $INSTALL_DIR/codex-chima-v1
Symlink:      /usr/local/bin/codex-chima-v1
CODEX_HOME:   $CODEX_HOME_DIR
Env file:     $ENV_FILE
Service file: $SERVICE_FILE
Listen URL:   $LISTEN_URL

The service was installed but not enabled or started.

Test CLI:
  sudo -u $SERVICE_USER CODEX_HOME=$CODEX_HOME_DIR $INSTALL_DIR/codex-chima-v1 --version

Start app-server:
  sudo systemctl start codex-chima-v1
  sudo systemctl status codex-chima-v1 --no-pager

Enable on boot, only after testing:
  sudo systemctl enable codex-chima-v1
EOF
