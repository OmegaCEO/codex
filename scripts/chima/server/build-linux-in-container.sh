#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../../.." && pwd)"
IMAGE_NAME="${IMAGE_NAME:-codex-chima-v1-builder:rust-1.95}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/server-artifacts/chima-v1}"
TARGET_DIR="${CARGO_TARGET_DIR:-/workspace/codex-rs/target-chima-linux}"

mkdir -p "$REPO_ROOT/.cargo-chima-container" "$REPO_ROOT/.chima-container-home"

docker build \
  -f "$SCRIPT_DIR/Dockerfile.build" \
  -t "$IMAGE_NAME" \
  "$SCRIPT_DIR"

docker run --rm \
  --user "$(id -u):$(id -g)" \
  -e CARGO_HOME=/workspace/.cargo-chima-container \
  -e CARGO_TARGET_DIR="$TARGET_DIR" \
  -e HOME=/workspace/.chima-container-home \
  -v "$REPO_ROOT:/workspace" \
  -w /workspace \
  "$IMAGE_NAME" \
  bash -c 'export PATH="/usr/local/cargo/bin:$PATH"; ./scripts/chima/build-codex-chima-v1-linux.sh'

mkdir -p "$ARTIFACT_DIR"
cp "$REPO_ROOT/codex-rs/target-chima-linux/release/codex-chima-v1" "$ARTIFACT_DIR/codex-chima-v1"
chmod +x "$ARTIFACT_DIR/codex-chima-v1"

printf '%s\n' "$ARTIFACT_DIR/codex-chima-v1"
