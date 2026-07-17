---
tags: [service, web]
status: active
updated: 2026-07-17
---

# Web UI (терминал)

## Назначение

Браузерный торговый терминал (стиль профессиональной платформы, ADR-012): верхняя панель метрик,
левый список инструментов, свечной график с таймфреймами, правая панель BUY/SELL, нижняя лента
сделок. На реальных данных [[gateway]]. Статический SPA (vanilla JS + TradingView Lightweight
Charts) за nginx. Каталог `web/`.

## Запуск

```bash
docker compose up --build   # gateway (внутренний) + web (nginx)
# → http://localhost:8888
```

## Компоненты и данные

- **Верх**: метрики BALANCE/EQUITY/FREE (из `/balance` котируемого актива текущего пользователя),
  переключатель Alice/Bob, статус WS.
- **Слева**: список пар — опрос `GET /instruments` каждую 1с (символ, change%, sell=bid, buy=ask, spread).
  Клик по строке → выбор инструмента. Поиск фильтрует список.
- **Центр**: тулбар таймфреймов (1M…1D) → `GET /candles/{id}?tf=`; свечи + объём; OHLC по курсору;
  водяной знак с символом. Live-обновление последней свечи из WS-сделок + периодический refetch (4с).
- **Справа**: вкладки OPEN DEAL (рынок) / LIMIT ORDER; лот-степпер; BUY/SELL с текущими ask/bid →
  `POST /orders` с Bearer-токеном.
- **Низ**: лента MARKET TRADES по всем парам из WS `/stream`.

Масштаб цен/объёмов — по `price_decimals`/`qty_decimals` из `/instruments` (хардкода нет).

## Связи

- Общается только с [[gateway]] по HTTP/WS через nginx-прокси (`web/nginx.conf`, один origin — без CORS).
- Основано на [[ADR-011-web-ui]], [[ADR-012-market-data-and-terminal-ui]].

## Ограничения / TODO

- Данные **реальные с Binance** (ADR-013); **сделки бумажные** ([[broker]], ADR-014).
- **Терминальные фичи:** таймфреймы 1M…1W; типы графика (Candles/Hollow/Bars/Line/Area);
  индикаторы (оверлеи SMA/EMA/Bollinger + осциллятор RSI/ATR/ADX/MACD, считаются на клиенте);
  price type (Mid/Ask/Bid); PIPS-режим; SL/TP-линии на графике; лимитные ордера (LIMIT ORDER →
  `/pending`); вкладки OPEN DEALS / LIMIT ORDERS / CLOSED DEALS / SIGNALS.
- **SIGNALS** — наши простые сигналы (RSI, MA-крест), **не Autochartist** (проприетарный, недоступен).
- Плечо/ликвидация, комиссии — [[backlog]].
- Только крипто; форекс/сырьё/акции/индексы — платный провайдер, [[backlog]].
- Токены Alice/Bob в UI (demo) — до настоящего auth. [[backlog]]
