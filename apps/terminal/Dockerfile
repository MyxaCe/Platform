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
COPY nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
