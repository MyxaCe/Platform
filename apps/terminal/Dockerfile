# Стадия 1: вендорим библиотеки из npm (без CDN в рантайме).
FROM node:20-alpine AS libs
WORKDIR /libs
RUN npm init -y >/dev/null 2>&1 && npm install lightweight-charts@4.2.0 cryptocurrency-icons@0.18.1

# Стадия 2: nginx со статикой.
FROM nginx:alpine
COPY --from=libs /libs/node_modules/lightweight-charts/dist/lightweight-charts.standalone.production.js \
     /usr/share/nginx/html/vendor/lightweight-charts.js
COPY --from=libs /libs/node_modules/cryptocurrency-icons/svg/color/ /usr/share/nginx/html/vendor/coins/
COPY public/ /usr/share/nginx/html/
# Конфиг как шаблон: entrypoint nginx прогоняет его через envsubst на старте и кладёт
# в conf.d/default.conf. Подставляем ТОЛЬКО FRAME_ANCESTORS (фильтр), чтобы не затронуть
# рантайм-переменные nginx ($host/$uri/$http_upgrade). ADR-023, фаза Т1.
COPY nginx.conf /etc/nginx/templates/default.conf.template
ENV FRAME_ANCESTORS="*" \
    NGINX_ENVSUBST_FILTER="FRAME_ANCESTORS"
EXPOSE 80
