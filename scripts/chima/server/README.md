# Codex CHIMA v1 Server Test Kit

This directory prepares a Linux server runtime for Codex CHIMA v1.

It does not deploy anything from Windows. Build and install on the target Ubuntu
server or inside a Linux build environment.

## Build

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config clang cmake
./scripts/chima/build-codex-chima-v1-linux.sh
```

Output:

```text
codex-rs/target-chima-linux/release/codex-chima-v1
```

## Build In A Container

On WS1 or another Linux host with Docker:

```bash
./scripts/chima/server/build-linux-in-container.sh
```

Output:

```text
server-artifacts/chima-v1/codex-chima-v1
```

The container build uses the repo checkout mounted at `/workspace`, keeps Cargo
cache under `.cargo-chima-container/`, and writes the Rust target directory under
`codex-rs/target-chima-linux/`.

## Install Service Files

```bash
sudo ./scripts/chima/server/install-codex-chima-v1.sh
```

Defaults:

```text
Binary:     /opt/codex-chima-v1/codex-chima-v1
CODEX_HOME: /var/lib/codex-chima-v1
User:       codex-chima
Listen:     ws://127.0.0.1:4222
Service:    codex-chima-v1.service
```

The installer writes the service and reloads systemd, but it does not enable or
start the service.

## Test

```bash
sudo -u codex-chima CODEX_HOME=/var/lib/codex-chima-v1 /opt/codex-chima-v1/codex-chima-v1 --version
sudo systemctl start codex-chima-v1
sudo systemctl status codex-chima-v1 --no-pager
```

## Safe Network Posture

The default listener is loopback-only:

```text
ws://127.0.0.1:4222
```

Keep it loopback-only for first tests. For remote testing, prefer Tailscale or a
locked-down reverse proxy with explicit auth. Do not expose app-server directly
on a public interface.

## Public GUI Staging

Install static GUI files and disabled reverse-proxy templates:

```bash
sudo ./scripts/chima/server/install-public-gui-staging.sh
```

Defaults:

```text
Static GUI:      /opt/codex-chima-v1/public-gui
Proxy templates: /opt/codex-chima-v1/proxy-templates
App-server path: /codex-chima-app/*
```

The installer does not enable a route. The GUI can check `/readyz` and perform a
WebSocket `initialize` handshake through a reverse proxy. The proxy must sit
behind an explicit access gate and strip the browser `Origin` header before
forwarding WebSocket traffic to the loopback app-server.

## Build And Install In One Step

```bash
./scripts/chima/server/build-and-install-codex-chima-v1.sh
```

Use this only after the plain build path is known to work on the server.
