#!/bin/sh
# Рендер рантайм-конфига терминала из env в отдаваемую статику (ADR-023).
# Запускается entrypoint'ом nginx до старта сервера. Подставляем только свои
# переменные, чтобы не задеть чужой синтаксис.
envsubst '${BRAND_URL} ${CABINET_ORIGIN}' < /config.js.template > /usr/share/nginx/html/config.js
