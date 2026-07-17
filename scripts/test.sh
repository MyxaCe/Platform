#!/usr/bin/env bash
# Прогон тестов ядра через Docker-образ Rust (см. ADR-004). Локальный Rust не нужен.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

MSYS_NO_PATHCONV=1 docker run --rm \
  -e CARGO_TARGET_DIR=/tmp/target \
  -v "${ROOT}:/app" -w /app \
  rust:slim cargo test --manifest-path core/Cargo.toml "$@"
