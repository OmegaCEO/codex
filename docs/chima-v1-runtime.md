# Codex CHIMA v1 Runtime

This is a non-deployed runtime plan for the CHIMA v1 Codex build.

## Windows CLI

The built Windows binary is:

```powershell
C:\cb\release\codex-chima-v1.exe
```

Run it with an isolated Codex home so it does not share sessions, global state, logs,
or app-server daemon files with the installed Codex app:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\chima\run-codex-chima-v1.ps1 --version
powershell -ExecutionPolicy Bypass -File .\scripts\chima\run-codex-chima-v1.ps1
```

The launcher sets:

```text
CODEX_HOME=%USERPROFILE%\.codex-chima-v1
```

The process name is still the executable filename, but the CLI internally identifies
as `codex` / `Codex CLI`.

## Windows Desktop Experiment

Do not replace the installed OpenAI Codex app. Prepare a copied runtime instead:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\chima\prepare-codex-chima-v1-desktop.ps1
```

The prepare script:

1. Finds the installed OpenAI Codex app package.
2. Copies its `app` directory to `%LOCALAPPDATA%\OpenAI\Codex-CHIMA-v1\app`.
3. Replaces only the copied app's `resources\codex.exe` with the CHIMA build.
4. Writes `%LOCALAPPDATA%\OpenAI\Codex-CHIMA-v1\Launch-Codex-CHIMA-v1.ps1`.
5. Sets the copied app launcher to use `%USERPROFILE%\.codex-chima-v1`.

This keeps the installed app and the normal `%USERPROFILE%\.codex` state untouched.

## Ubuntu / Server

The Windows `.exe` does not run natively on Ubuntu. Build a Linux binary on Ubuntu,
WSL, or a Linux build server:

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config clang cmake
./scripts/chima/build-codex-chima-v1-linux.sh
```

That produces:

```text
codex-rs/target-chima-linux/release/codex-chima-v1
```

For WS1, prefer the container build path so the Linux build dependencies stay
isolated from the host:

```bash
./scripts/chima/server/build-linux-in-container.sh
```

That produces:

```text
server-artifacts/chima-v1/codex-chima-v1
```

To install a test service without starting it:

```bash
sudo ./scripts/chima/server/install-codex-chima-v1.sh
```

Default server install paths:

```text
Binary:     /opt/codex-chima-v1/codex-chima-v1
CODEX_HOME: /var/lib/codex-chima-v1
User:       codex-chima
Service:    codex-chima-v1.service
Listen:     ws://127.0.0.1:4222
```

A server can run the CLI or app-server side:

```bash
export CODEX_CHIMA_BINARY=/opt/codex-chima-v1/codex-chima-v1
./scripts/chima/run-codex-chima-v1-linux.sh --version
./scripts/chima/run-codex-chima-v1-linux.sh app-server --listen ws://127.0.0.1:4222
```

Use a separate `CODEX_HOME`, such as:

```bash
export CODEX_HOME=/var/lib/codex-chima-v1
```

Server use is practical for CLI, `codex exec`, `codex app-server`, and daemon-style
remote-control experiments. The Electron desktop shell is not the server target.

See `scripts/chima/server/README.md` for the server test kit.

## Public GUI Staging

The server kit includes a staged public GUI shell:

```bash
sudo ./scripts/chima/server/install-public-gui-staging.sh
```

It installs static files under `/opt/codex-chima-v1/public-gui` and disabled Caddy
and Nginx examples under `/opt/codex-chima-v1/proxy-templates`.

Do not expose the loopback app-server directly. Put the GUI behind Cloudflare
Access, Tailscale/VPN-only ingress, or a reverse proxy auth gate. The proxy path
`/codex-chima-app/*` should forward to `http://127.0.0.1:4222/` and strip the
browser `Origin` header before WebSocket forwarding, because app-server rejects
browser-origin WebSockets by design.
