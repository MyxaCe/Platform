---
tags: [service, gateway]
status: active
updated: 2026-07-17
---

# Gateway

## Назначение

Сетевой шлюз биржи: REST для команд/чтения + WS для живой ленты событий, поверх [[orchestrator]]
(ADR-010). Первый живой сервис в Docker. Крейт `gateway/` (lib + bin). Внешние зависимости:
`tokio`, `axum`, `serde`. Доменные типы наружу не протекают — на границе локальные DTO.

## Запуск

```bash
docker compose up --build        # поднять сервис
# → http://localhost:8888  (внутри контейнера :8080; 8888 — т.к. 8080–8282 заняты WinNAT)
docker compose down              # остановить
```

Демо-данные при старте (`seed_demo`): инструмент BTC-USDT (id 1, base=1/BTC, quote=2/USDT),
пользователи с токенами `alice-token` / `bob-token` и депозитами.

## Эндпоинты

| Метод | Путь | Auth | Назначение |
|---|---|---|---|
| GET | `/health` | — | проверка живости |
| POST | `/admin/users` | — (dev) | создать пользователя `{token}` → `{user_id}` |
| POST | `/admin/deposit` | — (dev) | пополнить счёт `{user_id, asset, amount}` |
| GET | `/instruments` | — | список пар с `last/change/bid/ask/high/decimals` |
| GET | `/candles/{instrument}?tf=SEC&limit=N` | — | свечи OHLCV таймфрейма |
| GET | `/depth/{instrument}` | — | стакан (order book) с Binance, кэш ~400мс |
| GET | `/stats/{instrument}` | — | изменение (%) по таймфреймам 1m…1w, кэш ~3с |
| POST | `/orders` | Bearer | разместить `{instrument, side, type, price?, qty, tif?}` → `{order_id, events}` |
| DELETE | `/orders/{id}?instrument=N` | Bearer | отменить |
| GET | `/book/{instrument}?depth=N` | — | снапшот стакана `{bids, asks}` |
| GET | `/balance/{asset}` | Bearer | баланс `{available, held}` |
| POST | `/deals` | Bearer | открыть сделку `{instrument, side, qty, sl?, tp?}` → позиция ([[broker]]) |
| GET | `/deals` | Bearer | открытые позиции с live P&L |
| POST | `/deals/{id}/close` | Bearer | закрыть позицию → realized P&L |
| GET | `/deals/closed` | Bearer | история закрытых сделок |
| POST | `/pending` | Bearer | лимитный ордер `{instrument, side, qty, price, sl?, tp?}` |
| GET | `/pending` | Bearer | отложенные ордера |
| DELETE | `/pending/{id}` | Bearer | отменить лимитный ордер |
| GET | `/account` | Bearer | `{balance, equity, used_margin, free_margin, open_pnl}` (центы) |

Фоновый монитор (`spawn_monitor`) каждые 500мс триггерит лимитные ордера и SL/TP по ценам фида.
| WS | `/stream` | — | живая лента событий (JSON) |

> ⚠️ При добавлении нового REST-пути **обязательно** добавить его в регекс-прокси `web/nginx.conf`
> (иначе 405/SPA вместо API). См. [[bug-log]] BUG-004.

## Market data — реальные с Binance (ADR-013)

Симуляции **нет**. Данные реальные, публичный Binance без ключа (модуль `gateway/src/feed.rs`):
- `/candles` → REST `api/v3/klines` (реальный OHLCV), кэш ~2с; таймфреймы 1m..1d.
- `/instruments` → WS `@ticker` (last/change/high/bid/ask по 30 парам).
- `/stream` (лента + live-цена) ← WS `@aggTrade` (реальные сделки).
- Топ-30 крипто-пар; у каждой свои `price_decimals`/`qty_decimals` (строки Binance → целые raw).
- Реконнект при обрыве. Проверено: ETH ≈ 1822 = Binance/TradingView.

Не-крипто рынки (форекс/сырьё/акции/индексы) — требуют платного провайдера, в [[backlog]].

Ошибки: `InsufficientFunds → 402`, `SelfTrade → 409`, `NotOwner → 403`, `Engine(reason) → 400`,
нет/битый токен → `401`.

## Состояние (`AppState`, за `Arc`)

- `orch: Arc<Mutex<Orchestrator>>` — единственный писатель; запрос коротко берёт лок.
- `users: Arc<Mutex<UserRegistry>>` — dev-реестр токен→UserId.
- `order_seq: AtomicU64` — выдача уникальных OrderId (уникальность — ответственность границы).
- `events_tx: broadcast::Sender<String>` — живая лента для WS.

## Связи

- Использует [[orchestrator]] (`place_limit/place_market/cancel/deposit/register_instrument/snapshot/balance`).
- `EventDto: From<&Event>` — конвертация событий [[matching-engine]] в JSON.
- Основано на [[ADR-010-gateway-stack]].

## Тесты

`gateway/tests/api.rs` — 7 тестов через `oneshot`: health, 401 без/с битым токеном, полный путь
сделки с движением балансов, снапшот стакана, self-trade → 409, рыночная покупка. Плюс живой
smoke-тест через `curl` (см. историю коммита).

## Ограничения / TODO

- **Auth — dev-уровень** (токен→UserId in-memory): нет хэшей/сессий/ротации. Настоящий auth — [[backlog]].
- Admin-эндпоинты без защиты (dev).
- Суммы в JSON — числами (большие `i128` теряют точность в JS). Кодировать строками — [[backlog]].
- WS отдаёт все события всем; нет подписки по инструменту/приватным событиям пользователя — [[backlog]].
