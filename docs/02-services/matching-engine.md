---
tags: [service, core]
status: active
updated: 2026-07-17
---

# Matching Engine

## Назначение

Сердце ядра. Принимает [[#Команды|команду]], проводит её через [[order-book]] и порождает
неизменяемый список [[#События|событий]]. Детерминирован: без времени, случайности и I/O.
Реализация: `core/src/engine.rs`. Единственная точка входа — `MatchingEngine::apply`.

## Состояние

- `book: OrderBook` — стакан.
- `seq: u64` — монотонный счётчик приоритета по времени (номер прихода заявки).
- `next_trade_id: u64` — монотонный счётчик id сделок.

Оба счётчика — из состояния движка, а не из часов → replay детерминирован ([[ADR-003-event-sourcing-and-determinism]]).

## Публичный API

- `MatchingEngine::new()`
- `apply(Command) -> Vec<Event>` ⭐ — применить команду.
- `book() -> &OrderBook` — только чтение (снапшоты/тесты/будущие проекции).

## Команды

Модуль `core/src/command.rs`:
- `Command::PlaceOrder { id, side, order_type, qty, tif }`
- `Command::CancelOrder { id }`

## События

Модуль `core/src/event.rs` (это будущий журнал-истина):
- `OrderAccepted { id }`
- `Trade { id, price, qty, taker, maker, taker_side }` — цена = цена maker'а
- `OrderResting { id, price, qty }`
- `OrderFilled { id }`
- `OrderCanceledRemainder { id, qty }` — IOC/рыночная без ликвидности
- `OrderCanceled { id }`
- `OrderRejected { id, reason }` — `reason ∈ { NonPositiveQty, UnknownOrder }`

## Дисциплина событий при размещении заявки

1. `OrderAccepted` (после валидации `qty > 0`);
2. по одному `Trade` на каждое исполнение (+ `OrderFilled` для полностью добитого maker'а);
3. финальная судьба taker'а:
   - остаток = 0 → `OrderFilled`;
   - остаток > 0, лимитная GTC → `OrderResting` (встаёт в стакан);
   - остаток > 0, IOC/рыночная → `OrderCanceledRemainder`.

## Связи

- Использует [[order-book]] (`cross`, `insert`, `cancel`) и [[domain-types]].
- Вход/выход: `Command` → `Event`. Наружу события пойдут через адаптеры (шлюз/WS) в следующих фазах.

## Инварианты

- Один и тот же вход → один и тот же выход (тест `engine_is_deterministic`).
- После обработки книга не скрещена (тест `book_never_stays_crossed_after_matching`).

## Тесты

`core/tests/matching.rs` — 15 тестов: постановка, полное/частичное исполнение, приоритет
цена→время, лимит без скрещивания, рыночные, IOC, отмена, валидация, детерминизм.

Запуск: `bash scripts/test.sh` (через Docker, [[ADR-004-docker-rust-toolchain]]).

## Ограничения / TODO (Фаза 2+)

- Нет проверки баланса/холдов — появится с [[services-index|Ledger]].
- Нет реестра инструментов (tick/lot/decimals), одна абстрактная книга.
- Типы заявок: пока Limit/Market + GTC/IOC. Дальше: FOK, post-only, stop и т.д. — [[backlog]].
