---
tags: [moc, services]
updated: 2026-07-17
---

# Индекс сервисов и модулей

Каждый модуль/сервис получает здесь строку и собственный doc по скелету из
[[how-to-use-these-docs]]. Это точка входа для поиска неисправного модуля.

| Модуль | Doc | Статус | Фаза |
|---|---|---|---|
| Matching Engine | [[matching-engine]] | ✅ active | 1 |
| Order Book | [[order-book]] | ✅ active | 1 |
| Instrument Registry | [[instrument-registry]] | ✅ active | 1 |
| Domain Types (money/order) | [[domain-types]] | ✅ active | 1 |
| Ledger / Accounts | _(появится)_ | ⏳ не начат | 2 |
| Auth / Users | _(появится)_ | ⏳ не начат | 2 |
| Order Gateway (API) | _(появится)_ | ⏳ не начат | 2 |

## Карта кода (Фаза 1)

```
core/
├── Cargo.toml
├── src/
│   ├── lib.rs              — фасад крейта (re-export публичного API)
│   ├── domain/
│   │   ├── money.rs        — Price, Qty, notional            → [[domain-types]]
│   │   ├── order.rs        — Side, OrderId, OrderType, TimeInForce
│   │   └── instrument.rs   — Instrument, InstrumentRegistry  → [[instrument-registry]]
│   ├── command.rs          — Command (вход ядра)
│   ├── event.rs            — Event, TradeId, RejectReason (выход ядра)
│   ├── book.rs             — OrderBook, Fill, DepthSnapshot   → [[order-book]]
│   └── engine.rs           — MatchingEngine.apply()/snapshot() → [[matching-engine]]
├── tests/
│   └── matching.rs         — 27 интеграционных тестов (+ фаззинг инвариантов)
└── examples/
    └── demo.rs             — печатает стакан «лестницей» (bash scripts/demo.sh)
```
