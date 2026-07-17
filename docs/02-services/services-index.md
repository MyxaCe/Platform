---
tags: [moc, services]
updated: 2026-07-17
---

# Индекс сервисов и модулей

Каждый модуль/сервис получает здесь строку и собственный doc по скелету из
[[how-to-use-these-docs]]. Это точка входа для поиска неисправного модуля.

| Модуль (крейт) | Doc | Статус | Фаза |
|---|---|---|---|
| Domain Types (`domain`) | [[domain-types]] | ✅ active | 1 |
| Matching Engine (`core`) | [[matching-engine]] | ✅ active | 1 |
| Order Book (`core`) | [[order-book]] | ✅ active | 1 |
| Instrument Registry (`domain`) | [[instrument-registry]] | ✅ active | 1 |
| Ledger (`ledger`) | [[ledger]] | ✅ active | 2 |
| Orchestrator (`orchestrator`) | [[orchestrator]] | ✅ active | 2b |
| Market Data (`marketdata`) | [[marketdata]] | 💤 dormant (не исп. в real-режиме) | 2d |
| Real feed Binance (`gateway/feed.rs`) | [[gateway]] | ✅ active | 2e |
| Broker бумажный (`broker`) | [[broker]] | ✅ active | 2f |
| Gateway REST+WS (`gateway`) | [[gateway]] | ✅ active | 2c |
| Web UI терминал (`web/`) | [[web-ui]] | ✅ active | 2d |
| Auth (в gateway, dev) | [[gateway]] | 🟡 dev-уровень | 2c |

## Карта кода (workspace)

```
domain/src/                 — общий словарь (крейт domain, ADR-007)
├── money.rs                — Price, Qty, Amount, notional      → [[domain-types]]
├── order.rs                — Side, OrderId, OrderType, TimeInForce
├── instrument.rs           — Instrument, InstrumentRegistry    → [[instrument-registry]]
└── account.rs              — UserId

core/                       — matching-ядро (крейт exchange_core)
├── src/
│   ├── command.rs          — Command (вход ядра)
│   ├── event.rs            — Event, TradeId, RejectReason (выход ядра)
│   ├── book.rs             — OrderBook, Fill, DepthSnapshot    → [[order-book]]
│   └── engine.rs           — MatchingEngine.apply()/snapshot() → [[matching-engine]]
├── tests/matching.rs       — 27 тестов (+ фаззинг инвариантов)
└── examples/demo.rs        — печатает стакан «лестницей» (bash scripts/demo.sh)

ledger/                     — счета/балансы (крейт ledger)     → [[ledger]]
├── src/lib.rs              — Ledger, Balance, LedgerError, settle_fill
└── tests/ledger.rs         — 11 тестов (сходимость, price improvement)

orchestrator/               — путь ордера (крейт orchestrator) → [[orchestrator]]
├── src/lib.rs              — Orchestrator: place_limit/place_market/cancel + STP
└── tests/order_path.rs     — 17 тестов (движение денег, рыночные, STP, сходимость)

marketdata/                 — свечи OHLCV (крейт marketdata)   → [[marketdata]]
├── src/lib.rs              — Candle, CandleStore (ingest/candles/seed)
└── tests/candles.rs        — 5 тестов

gateway/                    — сетевой шлюз (крейт gateway)     → [[gateway]]
├── src/lib.rs              — AppState, роутер, DTO, хендлеры, auth, WS, свечи, симулятор
├── src/main.rs             — бинарь: seed_demo + seed_market + serve на :8080
├── Dockerfile              — release-сборка → тонкий образ
└── tests/api.rs            — 7 API-тестов (oneshot)

web/                        — терминальный UI (nginx + JS)     → [[web-ui]]
├── public/{index.html,app.js,style.css}  — метрики + список пар + график(TF) + BUY/SELL + лента
├── nginx.conf              — статика + прокси API/WS на gateway
└── Dockerfile              — вендор lightweight-charts → nginx
```

Итого тестов: **75** (core 27 + ledger 11 + orchestrator 17 + gateway 7 + marketdata 5 + broker 8).
Запуск тестов: `bash scripts/test.sh`. Запуск сервиса: `docker compose up --build` → http://localhost:8888.
