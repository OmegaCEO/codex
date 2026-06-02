#!/usr/bin/env bash
set -euo pipefail

BINARY_PATH="${CODEX_CHIMA_BINARY:-/opt/codex-chima-v1/codex-chima-v1}"
export CODEX_HOME="${CODEX_HOME:-$HOME/.codex-chima-v1}"

if [[ ! -x "$BINARY_PATH" ]]; then
  echo "CHIMA Codex binary not executable: $BINARY_PATH" >&2
  exit 1
fi

exec "$BINARY_PATH" "$@"
