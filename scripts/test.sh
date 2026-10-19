#!/usr/bin/env bash
# Прогон тестов Rust-workspace (backend/) через Docker-образ Rust (ADR-004).
# Кэш сборки и реестра crates.io — в именованных volume'ах (первый прогон медленный).
# Тома переименованы под текущий проект (были platform-target/platform-registry от
# допивотного репозитория). Старые НЕ удалены — решение по ним за владельцем; из-за
# смены имени первый прогон после этой правки собирает всё заново.
#
# Контракты CMS (TRM-15, Р-058). Эталонная таблица случаев видимости лежит в
# чужом репозитории, и у нас есть её копия. Копия без сверки — седьмой
# производный список, поэтому тест сверяет её с источником по содержимому, а
# источник надо ему дать: репозитории разные, общего дерева у них нет.
# Монтируется **только на чтение** — чужая папка, править её мы не вправе.
# Если каталога нет, переменная не задаётся вовсе, и тест сверки падает с
# «сверка НЕ выполнена»: непроверенное не должно выглядеть пройденным.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

CMS_CONTRACTS="${CMS_CONTRACTS:-${ROOT}/../cms/src/contracts}"
CONTRACTS_ARGS=()
if [ -d "${CMS_CONTRACTS}" ]; then
  CONTRACTS_ARGS=(-v "${CMS_CONTRACTS}:/contracts:ro" -e CMS_CONTRACTS_DIR=/contracts)
else
  echo "[test.sh] каталог контрактов CMS не найден (${CMS_CONTRACTS}) — сверка эталонной таблицы не выполнится и покраснеет" >&2
fi

MSYS_NO_PATHCONV=1 docker run --rm \
  -e CARGO_TARGET_DIR=/target \
  -v "${ROOT}/backend:/app" -w /app \
  -v terminal-target:/target \
  -v terminal-registry:/usr/local/cargo/registry \
  "${CONTRACTS_ARGS[@]}" \
  rust:slim sh -c "cargo test $*"
