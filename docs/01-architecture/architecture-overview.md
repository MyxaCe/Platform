---
tags: [architecture]
status: draft
updated: 2026-07-17
---

# Обзор архитектуры

> [!note] Черновик
> Наполняется по мере реализации фаз. Сейчас зафиксирована целевая картина и Фаза 1.

## Целевая картина (полная биржа)

```
                 ┌──────────────────────────────────────────────┐
  Клиент ──► API / Order Gateway ──(команда: PlaceOrder)──►      │
                                                       ЖУРНАЛ    │  событий
  ◄── WS Fanout ◄── Market Data ◄──(событие: Trade/BookΔ)──      │  (истина)
                    Ledger        ◄──(событие: Fill)─────────    │
                    History       ◄──(событие: Trade)────────    │
                 └──────────────────────────────────────────────┘
```

Сервисы (по [[development-principles]] — режем по реальным швам, не заранее):

| Сервис | Ответственность | Фаза |
|---|---|---|
| **Matching Engine** | стакан в памяти, матчинг, генерация сделок | 1 |
| **Order Book** (внутри ядра) | структура книги заявок, приоритет цена/время | 1 |
| **Ledger / Accounts** | балансы, холды, двойная запись | 2 |
| **Auth / Users** | аккаунты, сессии, ключи | 2 |
| **API / Order Gateway** | приём/валидация ордеров, risk-check | 2 |
| **Market Data Distribution** | раздача стакана/сделок/тиков по WS | 2–3 |
| **History / Storage** | сделки, свечи, ордера в БД | 3 |
| **Event Log** (Kafka/NATS) | упорядоченный журнал-истина | 3 |

## Структура крейтов (Rust workspace, ADR-007)

```
domain  — общий словарь (Price, Qty, Amount, Side, Order*, Instrument*, UserId)
  ├── core   (exchange_core) — matching: order book + engine (Command → Event)
  ├── ledger                 — счета/балансы: available/held, двойная запись
  └── orchestrator           — путь ордера: резерв → матчинг → расчёт (зависит от core + ledger)
core и ledger независимы; связывает их orchestrator (ADR-008).
```

## Сделано (Фаза 1 + Фаза 2)

- **core**: детерминированный matching engine, order book (price-time priority), реестр
  инструментов с валидацией, снапшот стакана. Вход `MatchingEngine::apply(Command) -> Vec<Event>`.
- **ledger**: `Ledger` с `reserve/release/settle_fill`, инвариант сходимости по активу.
- **orchestrator**: полный путь заявки с движением денег — `place_limit`/`cancel`
  (резерв до матчинга, расчёт по событиям, возврат резерва при отмене/отказе).

Принципы: [[ADR-002-money-representation]], [[ADR-003-event-sourcing-and-determinism]],
[[ADR-006-ledger-double-entry]], [[ADR-008-orchestrator]].

## Потоки данных Фазы 1

```
PlaceOrder(command) ─► MatchingEngine.apply(state, cmd)
                          │
                          ├─► events: OrderAccepted, Trade[], OrderResting/Filled/Canceled
                          └─► new state (order book)
```

Детали реализации модулей появятся в [[services-index]] по мере написания кода.
