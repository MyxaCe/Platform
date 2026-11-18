#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

IMAGE="${1:-}"
TAG="terminal-config-check:$$"
NAME="terminal-config-check-$$"
BRAND="https://brand.check.invalid/{site}/brand"
CABINET="https://cabinet.check.invalid"

cleanup() {
  docker rm -f "${NAME}" >/dev/null 2>&1 || true
  [ -n "${IMAGE}" ] || docker image rm -f "${TAG}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

if [ -z "${IMAGE}" ]; then
  echo "[check-config-js] собираю образ терминала"
  docker build -q -t "${TAG}" "${ROOT}/apps/terminal" >/dev/null
  IMAGE="${TAG}"
fi

docker run -d --name "${NAME}" --add-host gateway:127.0.0.1 \
  -e BRAND_URL="${BRAND}" -e CABINET_ORIGIN="${CABINET}" "${IMAGE}" >/dev/null

for _ in $(seq 1 30); do
  if docker exec "${NAME}" sh -c 'wget -qO- http://127.0.0.1/ >/dev/null 2>&1'; then break; fi
  sleep 1
done

log="$(docker logs "${NAME}" 2>&1 || true)"
if printf '%s' "${log}" | grep -q 'Ignoring .*40-terminal-config.sh'; then
  echo "[check-config-js] ОТКАЗ: entrypoint пропустил 40-terminal-config.sh как неисполняемый." >&2
  echo '[check-config-js] Лечится COPY --chmod=0755 в apps/terminal/Dockerfile.' >&2
  exit 1
fi

body="$(docker exec "${NAME}" sh -c 'wget -qO- http://127.0.0.1/config.js' || true)"

if printf '%s' "${body}" | grep -qi '<!DOCTYPE html\|<html'; then
  echo "[check-config-js] ОТКАЗ: /config.js отдаёт HTML — файла в статике нет, сработал try_files." >&2
  printf '%s\n' "${body}" | head -5 >&2
  exit 1
fi
if ! printf '%s' "${body}" | grep -q 'window.TERMINAL_CONFIG'; then
  echo "[check-config-js] ОТКАЗ: в /config.js нет window.TERMINAL_CONFIG." >&2
  printf '%s\n' "${body}" | head -5 >&2
  exit 1
fi
for expected in "${BRAND}" "${CABINET}"; do
  if ! printf '%s' "${body}" | grep -qF "${expected}"; then
    echo "[check-config-js] ОТКАЗ: значение из окружения не доехало в /config.js: ${expected}" >&2
    printf '%s\n' "${body}" | tail -5 >&2
    exit 1
  fi
done

echo "[check-config-js] ок: /config.js — JS, значения окружения на месте."
