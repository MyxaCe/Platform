# Вход и регистрация — отдельный продукт на поддомене accounts (ADR-019).
FROM nginx:alpine
COPY shared/ /usr/share/nginx/html/shared/
COPY accounts/public/ /usr/share/nginx/html/
COPY accounts/nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
