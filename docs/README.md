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

### Сервисы и модули
- [[services-index]] — список всех сервисов, у каждого свой doc

### Эксплуатация проекта
- [[bug-log]] — журнал багов (симптом → причина → фикс → регресс-тест)
- [[backlog]] — идеи и дальнейшие улучшения

## 📌 Статус проекта

| | |
|---|---|
| Фаза | **2 — деньги** ✅ Ledger + оркестратор (лимитные + рыночные, STP); 55 тестов + clippy |
| Стек | Rust workspace: `domain` + `core` + `ledger` + `orchestrator`; тулчейн через Docker |
| Следующий шаг | Auth + Order Gateway (сетевой слой, первый живой сервис в Docker) → Фаза 3 (журнал событий) |
| Дата | 2026-07-17 |

## 🗺️ Дорожная карта (фазы)

1. **Фаза 1** ✅ — Matching Engine + Order Book + инструменты + снапшот
2. **Фаза 2** ✅ — Ledger (балансы/холды/двойная запись) + оркестратор (путь ордера) · далее Auth + Gateway
3. **Фаза 3** — Журнал событий (Kafka/NATS) + распил на сервисы
4. **Фаза 4** — Реальное custody (безопасность + юр. проработка)
