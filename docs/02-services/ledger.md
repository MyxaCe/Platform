---
tags: [service, core]
status: active
updated: 2026-07-17
---

# Ledger

## Назначение

Счета и балансы биржи с двойной записью (ADR-006). Хранит деньги пользователей: свободные
(`available`) и зарезервированные под заявки (`held`). Все операции балансовы — по каждому активу
ничего не создаётся и не исчезает, кроме явных депозита/вывода. Отдельный крейт `ledger/`,
зависит от [[domain-types|domain]], **не** зависит от matching-ядра. Реализация: `ledger/src/lib.rs`.

## Модель

- Счёт = `(UserId, AssetId)` → `Balance { available, held }`.
- Суммы — `Amount(i128)`.
- Инвариант: `total_supply(asset)` постоянна между депозитами/выводами.

## Публичный API

- `Ledger::new()`
- Чтение: `balance/available/held(user, asset)`, `total_supply(asset)`
- `deposit(user, asset, Amount)` — внешнее пополнение (меняет supply)
- `withdraw(...) -> Result` — вывод (`InsufficientAvailable`)
- `reserve(...) -> Result` — `available → held` (при постановке заявки)
- `release(...) -> Result` — `held → available` (при отмене/остатке)
- `settle_fill(base, quote, buyer, seller, price, qty, buyer_reserve_price) -> Result` ⭐ — расчёт сделки

### Расчёт сделки (`settle_fill`)

Списывает из `held`, переводит контрагенту, возвращает покупателю price improvement
`(reserve − fill) × qty`. Сходится по обоим активам (доказательство — в [[ADR-006-ledger-double-entry]]).

## Связи

- Использует [[domain-types]] (`UserId`, `AssetId`, `Price`, `Qty`, `Amount`).
- **Пока не связан** с [[matching-engine]] напрямую. Их свяжет оркестратор Фазы 2b: `reserve` до
  матчинга → `settle_fill`/`release` по событиям `Trade`/`OrderCanceled`.
- Ошибки: `LedgerError { InsufficientAvailable, InsufficientHeld }`.

## Инварианты

- `available ≥ 0`, `held ≥ 0`; сумма по активу сохраняется (тесты `*_conserves*`, `conservation_holds*`).
- Ошибочная операция не меняет состояние (проверяется тестами).

## Тесты

`ledger/tests/ledger.rs` — 11 тестов: депозит/вывод, резерв/возврат, расчёт сделки (в т.ч.
price improvement), отказ без резерва, сходимость по серии операций.

## Ограничения / TODO (Фаза 2b+)

- Нет оркестрации с матчингом (резерв под заявку, расчёт по событиям).
- Резерв под **рыночную** заявку (цена заранее неизвестна). См. [[backlog]].
- Нет журнала событий Ledger'а (движения по счетам) — появится с event sourcing (Фаза 3).
