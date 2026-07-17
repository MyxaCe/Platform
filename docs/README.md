---
tags: [moc, home]
updated: 2026-07-17
---

# 🏛️ Platform — База знаний проекта

Это **Obsidian-хранилище** проекта «своя биржа/брокер». Здесь живёт вся документация:
принципы, архитектурные решения, описания сервисов, баги, планы.

> [!info] Как пользоваться
> Открой эту папку (`docs/`) как vault в Obsidian. Навигация — по ссылкам ниже и по графу.
> Правила ведения документации: [[how-to-use-these-docs]].

## 🧭 Карта (Map of Content)

### Основы
- [[development-principles]] — **принципы разработки** (красные линии, архитектура, код, тесты, процесс)
- [[how-to-use-these-docs]] — как ведём и структурируем документацию

### Архитектура
- [[architecture-overview]] — общая картина: сервисы, потоки данных, event sourcing
- Решения (ADR):
  - [[ADR-000-template]] — шаблон
  - [[ADR-001-language-and-stack]] — язык и стек ядра · `accepted` (**Rust**)
  - [[ADR-002-money-representation]] — представление денег · `accepted`
  - [[ADR-003-event-sourcing-and-determinism]] — событийность и детерминизм · `accepted`
  - [[ADR-004-docker-rust-toolchain]] — тулчейн Rust через Docker · `accepted`
  - [[ADR-005-instrument-model]] — модель инструмента и мульти-инструментный матчинг · `accepted`
  - [[ADR-006-ledger-double-entry]] — Ledger: счета, балансы, двойная запись · `accepted`
  - [[ADR-007-workspace-crate-structure]] — структура крейтов: общий `domain` · `accepted`
  - [[ADR-008-orchestrator]] — путь ордера: резерв → матчинг → расчёт · `accepted`
  - [[ADR-009-market-orders-and-stp]] — рыночные заявки и предотвращение self-trade · `accepted`
  - [[ADR-010-gateway-stack]] — стек сетевого шлюза (tokio + axum + serde) · `accepted`
  - [[ADR-011-web-ui]] — веб-интерфейс (живой стакан + график) · `accepted`
  - [[ADR-012-market-data-and-terminal-ui]] — свечи, несколько пар, терминальный UI · `accepted`
  - [[ADR-013-real-market-data-broker-pivot]] — реальные данные Binance, разворот к брокер-терминалу · `accepted`

### Сервисы и модули
- [[services-index]] — список всех сервисов, у каждого свой doc

### Эксплуатация проекта
- [[bug-log]] — журнал багов (симптом → причина → фикс → регресс-тест)
- [[backlog]] — идеи и дальнейшие улучшения

## 📌 Статус проекта

| | |
|---|---|
| Фаза | **2e — реальные данные** ✅ Binance (топ-30 крипто), симуляция убрана; 67 тестов + clippy |
| Стек | Rust: `domain`+`core`+`ledger`+`orchestrator`+`gateway` (reqwest/tungstenite→Binance) · web: nginx + JS |
| Запуск | `docker compose up --build` → **http://localhost:8888** (реальные крипто-цены Binance) |
| Следующий шаг | Broker-исполнение (позиции/маржа/P&L) · не-крипто рынки (платный провайдер) · настоящий auth |
| Дата | 2026-07-17 |

## 🗺️ Дорожная карта (фазы)

1. **Фаза 1** ✅ — Matching Engine + Order Book + инструменты + снапшот
2. **Фаза 2** ✅ — Ledger + оркестратор (путь ордера) + Gateway REST/WS (живой сервис в Docker)
3. **Фаза 3** — Журнал событий (Kafka/NATS) + распил на сервисы; настоящий auth
4. **Фаза 4** — Реальное custody (безопасность + юр. проработка)
