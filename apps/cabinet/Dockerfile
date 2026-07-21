# Личный кабинет клиента. Контекст сборки — apps/, чтобы забрать общие модули (ADR-017).
FROM nginx:alpine
COPY shared/ /usr/share/nginx/html/shared/
COPY cabinet/public/ /usr/share/nginx/html/
COPY cabinet/nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
