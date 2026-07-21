#!/usr/bin/env bash
# Запуск демо ядра (печатает стакан «лестницей») через Docker-образ Rust (ADR-004).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

MSYS_NO_PATHCONV=1 docker run --rm \
  -e CARGO_TARGET_DIR=/tmp/target \
  -v "${ROOT}/backend:/app" -w /app \
  rust:slim cargo run --quiet --manifest-path core/Cargo.toml --example demo
