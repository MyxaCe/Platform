---
tags: [service, core]
status: active
updated: 2026-07-17
---

# Instrument Registry

## Назначение

Описывает торговые инструменты (пары) и их правила. Реестр — конфиг-состояние движка:
инструменты регистрируются до начала торгов. От параметров инструмента зависят валидация
заявок и маршрутизация по книгам. Реализация: `core/src/domain/instrument.rs` (ADR-005).

## Публичный API

- `struct AssetId(pub u32)` — актив (BTC, USDT…); пока опаковый id (полный реестр активов — позже).
- `struct InstrumentId(pub u32)` — инструмент (пара).
- `struct Instrument { id, symbol, base, quote, price_decimals, qty_decimals, tick_size, lot_size, min_qty }`
- `struct InstrumentRegistry`
  - `new()`
  - `register(Instrument)` — валидирует параметры (`tick_size`/`lot_size`/`min_qty` > 0, `min_qty % lot_size == 0`)
  - `get(InstrumentId) -> Option<&Instrument>`
  - `contains(InstrumentId) -> bool`

## Правила валидации заявки (применяет [[matching-engine]])

Порядок и причина отказа:
1. `UnknownInstrument` — инструмент не зарегистрирован
2. `NonPositiveQty` — `qty <= 0`
3. `QtyNotOnLot` — `qty % lot_size != 0`
4. `BelowMinQty` — `qty < min_qty`
5. `NonPositivePrice` — цена лимитной `<= 0`
6. `PriceNotOnTick` — `price % tick_size != 0`

## Связи

- Использует [[domain-types]] (`Price`, `Qty`).
- Используется [[matching-engine]]: движок хранит реестр + по одной книге ([[order-book]]) на инструмент.
- Основано на [[ADR-005-instrument-model]].

## Инварианты

- Параметры зарегистрированного инструмента строго положительны; `min_qty` кратен `lot_size`.

## Ограничения / TODO

- Нет реестра активов (`AssetId` опаковый). Нет статусов инструмента (trading/halt), максимумов,
  цено-объёмных лимитов. См. [[backlog]].
