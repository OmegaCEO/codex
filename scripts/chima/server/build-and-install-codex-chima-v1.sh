#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../../.." && pwd)"

"$REPO_ROOT/scripts/chima/build-codex-chima-v1-linux.sh"

BINARY_PATH="${BINARY_PATH:-$REPO_ROOT/codex-rs/target-chima-linux/release/codex-chima-v1}"

if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
  BINARY_PATH="$BINARY_PATH" "$SCRIPT_DIR/install-codex-chima-v1.sh"
else
  sudo BINARY_PATH="$BINARY_PATH" "$SCRIPT_DIR/install-codex-chima-v1.sh"
fi
