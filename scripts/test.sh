#!/usr/bin/env bash
# Прогон тестов Rust-workspace (backend/) через Docker-образ Rust (ADR-004).
# Кэш сборки и реестра crates.io — в именованных volume'ах (первый прогон медленный).
# Тома переименованы под текущий проект (были platform-target/platform-registry от
# допивотного репозитория). Старые НЕ удалены — решение по ним за владельцем; из-за
# смены имени первый прогон после этой правки собирает всё заново.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

MSYS_NO_PATHCONV=1 docker run --rm \
  -e CARGO_TARGET_DIR=/target \
  -v "${ROOT}/backend:/app" -w /app \
  -v terminal-target:/target \
  -v terminal-registry:/usr/local/cargo/registry \
  rust:slim sh -c "cargo test $*"
