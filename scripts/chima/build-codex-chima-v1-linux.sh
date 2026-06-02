#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../.." && pwd)"
CODEX_RS="$REPO_ROOT/codex-rs"
TARGET_DIR="${CARGO_TARGET_DIR:-$CODEX_RS/target-chima-linux}"

cd "$CODEX_RS"
export CARGO_TARGET_DIR="$TARGET_DIR"

cargo build --release -p codex-cli --bin codex
cp "$TARGET_DIR/release/codex" "$TARGET_DIR/release/codex-chima-v1"
chmod +x "$TARGET_DIR/release/codex-chima-v1"

"$TARGET_DIR/release/codex-chima-v1" --version
printf '%s\n' "$TARGET_DIR/release/codex-chima-v1"
