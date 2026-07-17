#!/usr/bin/env bash
# Прогон тестов всего workspace (domain + core + ledger) через Docker-образ Rust (ADR-004).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

MSYS_NO_PATHCONV=1 docker run --rm \
  -e CARGO_TARGET_DIR=/tmp/target \
  -v "${ROOT}:/app" -w /app \
  rust:slim cargo test "$@"
