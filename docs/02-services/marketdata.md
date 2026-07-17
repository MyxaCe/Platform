---
tags: [service, marketdata]
status: active
updated: 2026-07-17
---

# Market Data (свечи)

## Назначение

Агрегация потока сделок в свечи (OHLCV) по таймфреймам (ADR-012). Чистая логика без async и
внешних зависимостей — время передаётся снаружи (боундари-сервис). Крейт `marketdata/`.

## Публичный API

- `struct Candle { time, open, high, low, close, volume }` — цены в raw-единицах, `time` = начало корзины (unix-сек).
- `struct CandleStore`
  - `new(tfs: Vec<u32>, cap)` — таймфреймы (сек) и лимит длины серии
  - `bucket(ts, tf) -> i64` — начало корзины
  - `ingest(instrument, ts, price, qty)` — учесть сделку во всех таймфреймах
  - `seed(instrument, tf, candles)` — заменить серию (бэкфилл истории)
  - `candles(instrument, tf, limit) -> Vec<Candle>` — последние `limit` свечей
  - `last_price(instrument, tf) -> Option<i64>`

## Связи

- Используется [[gateway]]: на каждую сделку `ingest`; эндпоинты `/candles` и `/instruments` читают.
- Основано на [[ADR-012-market-data-and-terminal-ui]].

## Тесты

`marketdata/tests/candles.rs` — 5 тестов: агрегация в одной корзине, новая корзина, независимость
таймфреймов, лимит-хвост, seed.

## Ограничения / TODO

- Свечи **в памяти** (не персистентны). Durable-журнал/хранилище — Фаза 3. [[backlog]]
- Ингест только вперёд по времени (сделки «из прошлого» игнорируются).
