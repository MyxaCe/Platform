---
tags: [moc, home]
updated: 2026-07-21
---

# 🏛️ Platform — База знаний проекта

Это **Obsidian-хранилище** проекта «своя биржа/брокер». Здесь живёт вся документация:
принципы, архитектурные решения, описания сервисов, баги, планы.

> [!info] Как пользоваться
> Открой эту папку (`docs/`) как vault в Obsidian. Навигация — по ссылкам ниже и по графу.
> Правила ведения документации: [[how-to-use-these-docs]].

## 🧭 Карта (Map of Content)

> [!tip] Возобновляешь работу?
> Начни с [[STATUS]] — где мы сейчас, структура репозитория и что дальше по приоритетам.

### Основы
- [[development-principles]] — **принципы разработки** · `accepted` (красные линии, архитектура, код, тесты, процесс)
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
  - [[ADR-014-paper-broker]] — бумажный брокер: позиции, маржа, P&L · `accepted`
  - [[ADR-015-reusable-frontend-modules]] — переиспользуемые фронтенд-модули · `accepted`
  - [[ADR-016-persistence-postgres]] — персистентность состояния: PostgreSQL · `accepted` (реализовано)
  - [[ADR-017-repo-layout-products]] — структура репозитория: продукты в `apps/`, ядро в `backend/` · `accepted`
  - [[ADR-018-authentication]] — аутентификация: Argon2id и серверные сессии · `accepted`
  - [[ADR-019-subdomains-and-stepped-login]] — поддомены продуктов и пошаговый вход · `accepted`

### Сервисы и модули
- [[services-index]] — список всех сервисов, у каждого свой doc

### Эксплуатация проекта
- [[06-operations]] — репозиторий, CI/CD, запуск на новой машине, переменные окружения
- [[bug-log]] — журнал багов (симптом → причина → фикс → регресс-тест)
- [[backlog]] — идеи и дальнейшие улучшения

## 📌 Статус проекта

| | |
|---|---|
| Фаза | **2h — продукты** · терминал готов, состояние durable (PostgreSQL); 86 тестов + clippy |
| Структура | `backend/` — ядро · `apps/`: edge, site, accounts, terminal, cabinet, shared (ADR-017/019) |
| Стек | Rust: `domain`+`core`+`ledger`+`orchestrator`+`broker`+`persistence`+`gateway` (→Binance) · web: nginx + JS |
| Запуск | `docker compose up --build` → **lvh.me:8888** · accounts / trade / my — поддомены |
| Следующий шаг | Пополнение/вывод и KYC в [[cabinet]] · плечо/ликвидация · техдолг в [[backlog]] |
| Дата | 2026-07-21 |

## 🗺️ Дорожная карта (фазы)

1. **Фаза 1** ✅ — Matching Engine + Order Book + инструменты + снапшот
2. **Фаза 2** ✅ — Ledger + оркестратор (путь ордера) + Gateway REST/WS (живой сервис в Docker)
3. **Фаза 2h** — продуктовый контур: сайт брокера, личный кабинет, интеграция с CRM (MICA)
4. **Фаза 3** — Журнал событий (Kafka/NATS) + распил на сервисы; настоящий auth
5. **Фаза 4** — Реальное custody (безопасность + юр. проработка)
