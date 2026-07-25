---
tags: [service, gateway]
status: active
updated: 2026-07-25
---

# Gateway

## Назначение

Сетевой шлюз терминала: REST для чтения данных и бумажной торговли + WS для живой ленты Binance.
Крейт `backend/gateway/` (lib + bin). Внешние зависимости: `tokio`, `axum`, `serde`, `reqwest`
(rustls), `tokio-tungstenite`. Доменные типы наружу не протекают — на границе локальные DTO.

После пивота (2026-07-25, [[ADR-022-standalone-terminal-pivot]]) шлюз автономный: **без логина**,
один счёт по умолчанию (`DEFAULT_USER = UserId(1)`, старт $100k). Слой аутентификации, matching и
funding сняты — как внешний сайт/CMS будет авторизовать пользователя, решается позже.

## Запуск

```bash
docker compose up --build        # postgres + gateway + terminal
# → http://localhost:8888  (терминал; внутри gateway слушает :8080)
docker compose down --remove-orphans
```

Демо-сида больше нет: счёт создаётся лениво при первой операции (или поднимается из БД на старте).

## Эндпоинты (все без авторизации, один счёт)

| Метод | Путь | Назначение |
|---|---|---|
| GET | `/health` | проверка живости |
| GET | `/instruments` | список пар с `last/change/bid/ask/high/decimals` |
| GET | `/candles/{instrument}?tf=SEC&limit=N` | свечи OHLCV таймфрейма |
| GET | `/depth/{instrument}?limit=N` | стакан с Binance, кэш ~400мс; N округляется вверх до набора Binance (20/50/100/500/1000) |
| GET | `/stats/{instrument}` | изменение (%) по таймфреймам 1m…1w, кэш ~3с |
| POST | `/deals` | открыть сделку `{instrument, side, qty, sl?, tp?}` → позиция по рыночной цене ([[broker]]) |
| GET | `/deals` | открытые позиции с live P&L |
| POST | `/deals/{id}/close` | закрыть позицию → realized P&L |
| GET | `/deals/closed` | история закрытых сделок |
| POST | `/pending` | лимитный ордер `{instrument, side, qty, price, sl?, tp?}` |
| GET | `/pending` | отложенные ордера |
| DELETE | `/pending/{id}` | отменить лимитный ордер |
| GET | `/account` | `{balance, equity, used_margin, free_margin, open_pnl}` (центы) |
| WS | `/stream` | живая лента событий Binance (JSON) |

Фоновый монитор (`spawn_monitor`) каждые 500мс триггерит лимитные ордера и SL/TP по ценам фида.

> ⚠️ При добавлении нового REST-пути **обязательно** добавить его в регекс-прокси
> `apps/terminal/nginx.conf` (иначе 405/SPA вместо API). См. [[bug-log]] BUG-004.

Ошибки: недостаточно маржи → `402`, неизвестный инструмент/позиция → `404`, битые параметры →
`400`, нет цены (фид ещё не прогрелся) → `503`, отказ хранилища → `503` (счёт откатывается).

## Market data — реальные с Binance (ADR-013)

Симуляции **нет**. Данные реальные, публичный Binance без ключа (модуль `backend/gateway/src/feed.rs`):
- `/candles` → REST `api/v3/klines` (реальный OHLCV), кэш ~2с; таймфреймы 1m..1w.
- `/instruments` → WS `@ticker` (last/change/high/bid/ask по 30 парам).
- `/stream` (лента + live-цена) ← WS `@aggTrade` (реальные сделки).
- Топ-30 крипто-пар; у каждой свои `price_decimals`/`qty_decimals` (строки Binance → целые raw).
- Реконнект при обрыве.

Не-крипто рынки (форекс/сырьё/акции/индексы) — требуют платного провайдера, в [[backlog]].

## Состояние (`AppState`, за `Arc`)

- `feed_instruments`, `tickers`, `klines_cache`, `depth_cache`, `stats_cache` — реальные данные Binance.
- `broker: Arc<Mutex<Broker>>` — бумажный брокер, единственный писатель счёта.
- `store: Arc<dyn Persistence>` — durable-хранилище счетов (ADR-016).
- `acct_locks` — лок на счёт: сериализует «мутация → слепок → запись» (`with_account`).
- `events_tx: broadcast::Sender<String>` — живая лента для WS.

## Дисциплина записи (`with_account`, ADR-016)

Мутация счёта: лок счёта на всё «мутация → слепок → запись»; `fsync`/сеть — уже без лока брокера;
при отказе хранилища счёт откатывается к прежнему слепку и отдаётся `503`. Монитор пишет только
реально изменившиеся счета.

## Связи

- Использует [[broker]] (`open/close/place_pending/cancel_pending/positions/snapshot/check/...`).
- Хранилище — крейт `persistence` ([[ADR-016-persistence-postgres]]).
- Стек — [[ADR-010-gateway-stack]] (matching-DTO из него сняты пивотом).

## Тесты

`backend/gateway/tests/api.rs` — 7 тестов через `oneshot` (без сети): health, дефолтный счёт $100k,
открытие сделки без цены → 503, неизвестный инструмент → 404, битый side → 400, размещение/список
отложенных, откат отложенного ордера при отказе хранилища → 503.

## Ограничения / TODO

- Один счёт (`DEFAULT_USER`). Мультипользовательность вернётся с интеграцией внешних сайтов/CMS.
- Суммы в JSON — числами (большие `i128` теряют точность в JS). Кодировать строками — [[backlog]].
- WS отдаёт все события всем; подписки по инструменту нет — [[backlog]].
