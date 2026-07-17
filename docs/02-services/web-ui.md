---
tags: [service, web]
status: active
updated: 2026-07-17
---

# Web UI

## Назначение

Браузерный интерфейс биржи: живой **стакан**, **лента сделок**, **свечной график** и **форма
заявки** — на реальных данных [[gateway]]. Статический SPA (vanilla JS + TradingView Lightweight
Charts), отдаётся nginx'ом, который проксирует API/WS на gateway. Каталог `web/`. ADR-011.

## Запуск

```bash
docker compose up --build     # gateway (внутренний) + web (nginx)
# → http://localhost:8888
```

## Устройство

- `web/public/index.html` — разметка: стакан | график | (форма + лента).
- `web/public/app.js`:
  - **график** — свечи строятся из потока сделок WS `/stream`, агрегируются по секундным корзинам
    (время — клиентское, ядро детерминировано и метки не шлёт);
  - **стакан** — опрос `GET /book/1?depth=12` каждые 700 мс, отрисовка с барами объёма;
  - **лента** — из trade-событий WS;
  - **форма** — `POST /orders` с Bearer-токеном (Alice/Bob);
  - **Авто-демо** — таймер шлёт заявки (alice продаёт, bob покупает — без self-trade), график
    двигается сам.
- `web/nginx.conf` — статика + прокси `/orders|/book|/balance|/admin|/health` и WS `/stream` на `gateway:8080`.
- `web/Dockerfile` — вендорит lightweight-charts из npm, кладёт в nginx.

Масштаб сумм на клиенте: `PRICE_SCALE=100` (price_decimals=2), `QTY_SCALE=1000` (qty_decimals=3).

## Связи

- Общается только с [[gateway]] по HTTP/WS (один origin через nginx-прокси — без CORS).
- Основано на [[ADR-011-web-ui]].

## Ограничения / TODO

- Свечи клиентские и по времени получения. Серверные свечи + история — с журналом (Фаза 3). [[backlog]]
- Инструмент/масштабы захардкожены (демо BTC-USDT). Позже — из `/admin/instruments` или публичного эндпоинта.
- Токены Alice/Bob прямо в UI (demo). Реальный вход — с настоящим auth. [[backlog]]
