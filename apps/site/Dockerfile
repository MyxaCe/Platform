# Сайт брокера — статика за nginx. Контекст сборки — каталог apps/, чтобы забрать
# общие модули (ADR-017): у продуктов общая машинерия i18n/темы, но не тексты.
FROM nginx:alpine
COPY shared/ /usr/share/nginx/html/shared/
COPY site/public/ /usr/share/nginx/html/
COPY site/nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
