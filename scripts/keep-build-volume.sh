#!/usr/bin/env bash
set -euo pipefail

VOLUME="terminal-target"
KEEPER="do-not-remove--holds-${VOLUME}--cold-rebuild-3.6gb"
IMAGE="rust:slim"

if ! docker info >/dev/null 2>&1; then
  echo "[keep-build-volume] Docker не отвечает — держатель НЕ создан." >&2
  exit 1
fi

if docker container inspect "${KEEPER}" >/dev/null 2>&1; then
  echo "[keep-build-volume] держатель уже есть: ${KEEPER}"
else
  MSYS_NO_PATHCONV=1 docker create \
    --name "${KEEPER}" \
    --label "purpose=Держит том ${VOLUME} от уборки: том выглядит кешем, но его потеря стоит холодной сборки" \
    --label "cost=3.6 ГБ артефактов; восстановление — холодная сборка; 20.10 она увела свободное место с 21 ГБ до 11" \
    --label "decision=Р-030 (расширено 21.10), задача TRM-39" \
    --label "never-runs=Контейнер создан и никогда не запускается; он нужен только как ссылка на том" \
    -v "${VOLUME}:/target:ro" \
    "${IMAGE}" \
    sh -c 'echo "Этот контейнер не предназначен для запуска. Он держит том terminal-target от уборки (TRM-39)."' \
    >/dev/null
  echo "[keep-build-volume] держатель создан: ${KEEPER}"
fi

if docker volume ls --filter dangling=true --format '{{.Name}}' | grep -qx "${VOLUME}"; then
  echo "[keep-build-volume] ⚠️ том ${VOLUME} ВСЁ ЕЩЁ числится ничейным — держатель не сработал" >&2
  exit 1
fi
echo "[keep-build-volume] том ${VOLUME} больше не числится ничейным — prune его не тронет"
