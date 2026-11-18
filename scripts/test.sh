#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

git -C "${ROOT}" config core.hooksPath scripts/hooks 2>/dev/null || true

if ! docker info >/dev/null 2>&1; then
  echo "[test.sh] Docker не отвечает — проверки НЕ выполнены (это не 'зелено')." >&2
  echo "[test.sh] Запустите Docker Desktop и повторите." >&2
  exit 1
fi

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
  -v "${ROOT}/scripts/target-volume-warning.txt:/warning.txt:ro" \
  "${CONTRACTS_ARGS[@]}" \
  rust:slim sh -c '
    set -eu
    # Предупреждение о цене потери кладём В САМ ТОМ, и кладём ЗАНОВО, если его
    # там нет (TRM-39). Иначе защита держалась бы на том, что кто-то однажды
    # положил файл руками: пересоздали том — и предупреждения нет ровно тогда,
    # когда оно нужнее всего. Текст лежит в репозитории, а не в этой строке,
    # потому что править его придётся читателям, а не скрипту.
    [ -f /target/00-НЕ-УДАЛЯТЬ-ЦЕНА-ПОТЕРИ.txt ] || cp /warning.txt /target/00-НЕ-УДАЛЯТЬ-ЦЕНА-ПОТЕРИ.txt
    cargo clippy --version >/dev/null 2>&1 || rustup component add clippy
    cargo fmt --version    >/dev/null 2>&1 || rustup component add rustfmt
    echo "[test.sh] тулчейн: $(rustc --version) | $(cargo clippy --version)"

    echo "[test.sh] --- тесты ---"
    cargo test --workspace "$@"

    echo "[test.sh] --- clippy ---"
    cargo clippy --workspace --all-targets --message-format=short -- -D warnings

    echo "[test.sh] --- формат ---"
    # Шаг БЛОКИРУЮЩИЙ с 21.10 (TRM-37), и ровно так же в `ci.yml`.
    #
    # История нужна, чтобы следующий не развязал их снова. Сперва (TRM-29) он
    # блокировал здесь и не блокировал в CI: команда была скопирована дословно,
    # а её обязательность — нет, и локальный прогон вышел СТРОЖЕ удалённого.
    # Расхождение в любую сторону означает «у себя и там по-разному».
    # Затем он не блокировал в обоих местах — честно, но стиль не выравнивался.
    # Теперь дерево выровнено целиком, и шаг обязателен в обоих местах.
    #
    # Эти две строки — здесь и в `ci.yml` — одно решение, записанное дважды.
    # Меняются ТОЛЬКО ВМЕСТЕ.
    cargo fmt --all -- --check

    echo "[test.sh] Rust: пройдено"
  ' sh "$@"

FRONT="${ROOT}/apps/terminal"
echo "[test.sh] --- фронт (npm test) ---"
if ! command -v npm >/dev/null 2>&1; then
  echo "[test.sh] npm не найден — ФРОНТОВЫЕ ПРОВЕРКИ НЕ ВЫПОЛНЕНЫ (это не 'зелено')." >&2
  exit 1
fi
if [ ! -d "${FRONT}/node_modules" ]; then
  echo "[test.sh] нет ${FRONT}/node_modules — фронтовые проверки НЕ ВЫПОЛНЕНЫ." >&2
  echo "[test.sh] Выполните: cd apps/terminal && npm ci" >&2
  exit 1
fi
( cd "${FRONT}" && npm test )

if [ "$#" -gt 0 ]; then
  echo "[test.sh] ⚠ ПРОГОН С ФИЛЬТРОМ ($*) — проверено НЕ ВСЁ, итог ниже относится только к отобранному"
fi
echo "[test.sh] ВСЕ ПРОВЕРКИ ПРОЙДЕНЫ"
