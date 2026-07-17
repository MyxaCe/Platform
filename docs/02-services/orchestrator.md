---
tags: [service, core]
status: active
updated: 2026-07-17
---

# Orchestrator

## Назначение

Связывает [[ledger]] (деньги) и [[matching-engine]] (матчинг) в полный путь заявки по схеме
**reserve-before-match, settle-from-events** (ADR-008). Гарантирует, что заявка не исполнится без
обеспечения. Отдельный крейт `orchestrator/` (зависит от `domain`, `exchange_core`, `ledger`).
Реализация: `orchestrator/src/lib.rs`.

## Публичный API

- `Orchestrator::new()`
- `register_instrument(Instrument)` — форвардит в движок.
- `deposit(user, asset, Amount)` — пополнить счёт (форвардит в ledger).
- `place_limit(user, instrument, id, side, price, qty, tif) -> Result<Vec<Event>, OrderReject>` ⭐
- `cancel(user, instrument, id) -> Result<Vec<Event>, OrderReject>`
- Чтение: `balance/available/held(user, asset)`, `total_supply(asset)`, `book(instrument)`,
  `snapshot(instrument, depth)`.
- `enum OrderReject { InsufficientFunds, Engine(RejectReason), NotOwner }`

## Путь заявки (`place_limit`)

1. **Резерв**: buy → `quote = цена×объём`, sell → `base = объём`. Не хватило → `InsufficientFunds`
   (движок не трогаем).
2. **Матчинг**: `engine.apply(PlaceOrder)`.
3. **Отказ движка** (tick/lot/min/инструмент) → вернуть резерв, `Engine(reason)`.
4. **Расчёт по событиям**: `Trade` → `settle_fill`; `OrderFilled` → забыть запись;
   `OrderCanceledRemainder`/`OrderCanceled` → вернуть остаток резерва.

Цена резерва покупателя в расчёте: taker-покупатель → его лимит (возврат разницы); maker-покупатель
→ цена сделки (возврата нет). См. [[ADR-008-orchestrator]].

## Состояние

- `engine: MatchingEngine`, `ledger: Ledger`
- `orders: HashMap<OrderId, {user, reserved_asset, reserved_remaining}>` — реестр живых заявок для
  маршрутизации расчётов и точного возврата резерва.

## Связи

- Использует [[matching-engine]] (`apply`, `instrument`, `book`, `snapshot`) и [[ledger]]
  (`reserve/release/settle_fill/deposit`).
- Основано на [[ADR-008-orchestrator]].

## Инварианты

- `total_supply(asset)` сохраняется на всём пути (тест `conservation_holds_across_full_scenario`).
- Отклонённая заявка не оставляет «повисшего» резерва (тесты на insufficient/rejection/cancel).

## Тесты

`orchestrator/tests/order_path.rs` — 10 тестов: резерв buy/sell, недостаток средств, полный матч
с движением денег, price improvement, частичное исполнение с остатком, отмена, отказ движка,
защита от отмены чужой заявки, сходимость.

## Ограничения / TODO

- Только лимитные заявки. Рыночные (резерв при неизвестной цене) — [[backlog]].
- Self-trade допускается (численно корректно) — предотвращение в [[backlog]].
- Нет журнала событий пути ордера — появится с event sourcing (Фаза 3).
