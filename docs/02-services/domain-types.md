---
tags: [service, domain]
status: active
updated: 2026-07-17
---

# Domain Types (крейт `domain`)

## Назначение

Общий доменный словарь, разделяемый сервисами ([[matching-engine]], [[ledger]] и будущими).
Только типы и реестр инструментов, без бизнес-логики. Отдельный крейт `domain/` (ADR-007).

## Публичный API

### `money.rs`
- `struct Price(pub i64)` — цена в минимальных единицах (ADR-002).
- `struct Qty(pub i64)` — объём; `ZERO`, `is_zero()`, `is_positive()`, `Add`/`Sub`.
- `struct Amount(pub i128)` — денежная сумма для балансов (шире `i64` под нотионал); `Add`/`Sub`,
  `is_zero/positive/negative`.
- `fn notional(Price, Qty) -> Amount` — цена × объём в `i128` без переполнения.

### `order.rs`
- `enum Side { Buy, Sell }` + `opposite()`
- `struct OrderId(pub u64)`
- `enum OrderType { Limit { price }, Market }`
- `enum TimeInForce { Gtc, Ioc }`

### `instrument.rs`
- `struct AssetId(pub u32)`, `struct InstrumentId(pub u32)`
- `struct Instrument { id, symbol, base, quote, price_decimals, qty_decimals, tick_size, lot_size, min_qty }`
- `struct InstrumentRegistry` — `register/get/contains` (валидирует параметры)

### `account.rs`
- `struct UserId(pub u64)` — владелец счетов в [[ledger]].

## Связи

- Используется [[matching-engine]], [[order-book]], [[instrument-registry]], [[ledger]].
- Основано на [[ADR-002-money-representation]], [[ADR-007-workspace-crate-structure]].

## Инварианты

- Деньги/цены/объёмы — только целые. `Price`, `Qty`, `Amount` не смешиваются (разные типы).
