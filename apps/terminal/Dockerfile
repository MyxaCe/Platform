FROM node:20-alpine AS libs
WORKDIR /libs
RUN npm init -y >/dev/null 2>&1 && npm install lightweight-charts@4.2.0 cryptocurrency-icons@0.18.1

FROM nginx:alpine
COPY --from=libs /libs/node_modules/lightweight-charts/dist/lightweight-charts.standalone.production.js \
     /usr/share/nginx/html/vendor/lightweight-charts.js
COPY --from=libs /libs/node_modules/cryptocurrency-icons/svg/color/ /usr/share/nginx/html/vendor/coins/
COPY public/ /usr/share/nginx/html/
COPY nginx.conf /etc/nginx/templates/default.conf.template
COPY config.js.template /config.js.template
COPY --chmod=0755 docker-entrypoint.d/40-terminal-config.sh /docker-entrypoint.d/40-terminal-config.sh
ENV FRAME_ANCESTORS="*" \
    NGINX_ENVSUBST_FILTER="FRAME_ANCESTORS" \
    BRAND_URL="" \
    CABINET_ORIGIN=""
EXPOSE 80
