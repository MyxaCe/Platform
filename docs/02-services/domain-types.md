---
tags: [service, domain]
status: active
updated: 2026-07-17
---

# Domain Types (money + order)

## Назначение

«Язык» предметной области биржи: типы для денег и заявок. Не содержат логики матчинга —
только описывают данные и их безопасные операции. Реализация: `core/src/domain/`.

## Публичный API

### `money.rs`
- `struct Price(pub i64)` — цена в минимальных единицах (ADR-002).
- `struct Qty(pub i64)` — объём в минимальных единицах.
  - `Qty::ZERO`, `is_zero()`, `is_positive()`
  - `impl Sub`/`Add` (с `debug_assert` на уход объёма в минус)
- `fn notional(Price, Qty) -> i128` — нотионал в `i128` (защита от переполнения; для Фазы 2).

### `order.rs`
- `enum Side { Buy, Sell }` + `Side::opposite()`
- `struct OrderId(pub u64)` — уникальный id, назначается снаружи ядра.
- `enum OrderType { Limit { price }, Market }`
- `enum TimeInForce { Gtc, Ioc }`

## Связи

- Используется всеми модулями ядра: [[order-book]], [[matching-engine]], командами и событиями.
- Основано на решении [[ADR-002-money-representation]].

## Инварианты

- Деньги/цены/объёмы — только целые. Никакого float.
- `Price` и `Qty` не смешиваются (разные типы).

## Ограничения / TODO

- `tick_size`/`lot_size`/`decimals` инструмента пока не заведены (появятся с реестром инструментов).
