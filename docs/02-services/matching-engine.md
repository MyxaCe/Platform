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

- `instruments: InstrumentRegistry` — реестр инструментов ([[instrument-registry]]).
- `books: HashMap<InstrumentId, OrderBook>` — по одной книге на инструмент (ADR-005).
- `seq: u64` — глобальный монотонный счётчик приоритета по времени (номер прихода заявки).
- `next_trade_id: u64` — глобальный монотонный счётчик id сделок.

Счётчики — из состояния движка, а не из часов → replay детерминирован ([[ADR-003-event-sourcing-and-determinism]]).

## Публичный API

- `MatchingEngine::new()`
- `register_instrument(Instrument)` — зарегистрировать инструмент и создать его пустую книгу (до торгов).
- `apply(Command) -> Vec<Event>` ⭐ — применить команду (маршрутизируется по `instrument`).
- `book(InstrumentId) -> Option<&OrderBook>` — только чтение (снапшоты/тесты/будущие проекции).

## Команды

Модуль `core/src/command.rs` (каждая команда несёт `instrument`):
- `Command::PlaceOrder { instrument, id, side, order_type, qty, tif }`
- `Command::CancelOrder { instrument, id }`

## События

Модуль `core/src/event.rs` (будущий журнал-истина; каждое событие несёт `instrument`):
- `OrderAccepted { instrument, id }`
- `Trade { instrument, id, price, qty, taker, maker, taker_side }` — цена = цена maker'а
- `OrderResting { instrument, id, price, qty }`
- `OrderFilled { instrument, id }`
- `OrderCanceledRemainder { instrument, id, qty }` — IOC/рыночная без ликвидности
- `OrderCanceled { instrument, id }`
- `OrderRejected { instrument, id, reason }` — причины валидации см. [[instrument-registry]]

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

`core/tests/matching.rs` — 22 теста: постановка, полное/частичное исполнение, приоритет
цена→время, лимит без скрещивания, рыночные, IOC, отмена, валидация (инструмент/tick/lot/min),
мульти-инструментная изоляция, детерминизм и **фаззинг инвариантов** (5000 операций, фикс. сид).

Запуск: `bash scripts/test.sh` (через Docker, [[ADR-004-docker-rust-toolchain]]).
Линт: `cargo clippy ... -- -D warnings` (чисто).

## Ограничения / TODO (Фаза 2+)

- Нет проверки баланса/холдов — появится с [[services-index|Ledger]].
- Типы заявок: пока Limit/Market + GTC/IOC. Дальше: FOK, post-only, stop и т.д. — [[backlog]].
- Нет снапшота стакана (глубина/уровни) для market data.
