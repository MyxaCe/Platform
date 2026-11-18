#!/bin/sh
envsubst '${BRAND_URL} ${CABINET_ORIGIN}' < /config.js.template > /usr/share/nginx/html/config.js
