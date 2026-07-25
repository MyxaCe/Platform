#!/usr/bin/env bash
# Прогон тестов Rust-workspace (backend/) через Docker-образ Rust (ADR-004).
# Кэш сборки и реестра crates.io — в именованных volume'ах (первый прогон медленный).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

MSYS_NO_PATHCONV=1 docker run --rm \
  -e CARGO_TARGET_DIR=/target \
  -v "${ROOT}/backend:/app" -w /app \
  -v platform-target:/target \
  -v platform-registry:/usr/local/cargo/registry \
  rust:slim sh -c "cargo test $*"
