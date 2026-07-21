# Сайт брокера — статика за nginx. Отдельный образ и контейнер: с торговым
# терминалом общего кода нет, и его статика сюда не попадает.
FROM nginx:alpine
COPY public/ /usr/share/nginx/html/
COPY nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
