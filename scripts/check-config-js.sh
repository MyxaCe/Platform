#!/usr/bin/env bash
# Проверка СОБРАННОГО образа терминала: `/config.js` отдаётся как конфиг, а не как
# index.html (B-OPS-016).
#
# Почему проверка на образе, а не на исходнике. Дефект жил ровно в зазоре между
# ними: в репозитории скрипт `docker-entrypoint.d/40-terminal-config.sh` лежит с
# режимом `100644`, образ nginx запускает из этого каталога ТОЛЬКО исполняемые
# файлы, а неисполняемые пропускает строкой «Ignoring ..., not executable» — то
# есть отказ выглядит обычной строкой лога. `/config.js` при этом не появляется
# в статике вовсе, и `try_files` отдаёт на него index.html: ответ 200, тело —
# HTML. Чтение Dockerfile этого не показывает.
#
# Почему проверка смотрит на ЗНАЧЕНИЕ, а не на наличие файла. Сегодня терминал
# работает правильно по совпадению: нужны пустые значения, и при отсутствии
# подстановки получаются пустые. Поэтому подставляем заведомо непустые и ищем
# именно их — иначе проверка была бы зелёной и со сломанным скриптом.
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

# `gateway` в nginx.conf — upstream для /stream и REST. Хоста тут нет, а nginx
# на старте резолвит upstream и без него падает на [emerg]. Подставляем заглушку:
# проверяем статику, а не проксирование.
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
# Значения из окружения обязаны доехать. Пустое значение здесь неотличимо от
# «подстановка не выполнилась» — ровно на этом дефект и прятался.
for expected in "${BRAND}" "${CABINET}"; do
  if ! printf '%s' "${body}" | grep -qF "${expected}"; then
    echo "[check-config-js] ОТКАЗ: значение из окружения не доехало в /config.js: ${expected}" >&2
    printf '%s\n' "${body}" | tail -5 >&2
    exit 1
  fi
done

echo "[check-config-js] ок: /config.js — JS, значения окружения на месте."
