---
tags: [moc, home]
updated: 2026-07-21
---

# 🏛️ Platform — База знаний проекта

Это **Obsidian-хранилище** проекта. Здесь живёт вся документация: принципы, архитектурные
решения, описания сервисов, баги, планы.

> [!warning] Пивот 2026-07-25 ([[ADR-022-standalone-terminal-pivot]])
> Проект стал **автономным торговым терминалом** для встраивания во внешние сайты, а не
> мультипродуктовой биржей. Часть ADR и сервис-доков ниже описывают снятый код (matching,
> аутентификация, продукты `apps/`) — они помечены `superseded`/`retired` и остаются как
> история. Актуальная картина — в [[STATUS]].

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
  - [[ADR-006-ledger-double-entry]] — Ledger: счета, балансы, двойная запись · `retired` (ADR-022)
  - [[ADR-007-workspace-crate-structure]] — структура крейтов: общий `domain` · `accepted`
  - [[ADR-008-orchestrator]] — путь ордера: резерв → матчинг → расчёт · `retired` (ADR-022)
  - [[ADR-009-market-orders-and-stp]] — рыночные заявки и предотвращение self-trade · `retired` (ADR-022)
  - [[ADR-010-gateway-stack]] — стек сетевого шлюза (tokio + axum + serde) · `accepted`
  - [[ADR-011-web-ui]] — веб-интерфейс (живой стакан + график) · `accepted`
  - [[ADR-012-market-data-and-terminal-ui]] — свечи, несколько пар, терминальный UI · `accepted`
  - [[ADR-013-real-market-data-broker-pivot]] — реальные данные Binance, разворот к брокер-терминалу · `accepted`
  - [[ADR-014-paper-broker]] — бумажный брокер: позиции, маржа, P&L · `accepted`
  - [[ADR-015-reusable-frontend-modules]] — переиспользуемые фронтенд-модули · `accepted`
  - [[ADR-016-persistence-postgres]] — персистентность состояния: PostgreSQL · `accepted` (реализовано)
  - [[ADR-017-repo-layout-products]] — структура репозитория: продукты в `apps/`, ядро в `backend/` · `superseded` (ADR-022)
  - [[ADR-018-authentication]] — аутентификация: Argon2id и серверные сессии · `superseded` (ADR-022)
  - [[ADR-019-subdomains-and-stepped-login]] — поддомены продуктов и пошаговый вход · `superseded` (ADR-022)
  - [[ADR-020-passkey-webauthn]] — Passkey / WebAuthn · `superseded` (ADR-022)
  - [[ADR-021-oauth-external-login]] — вход через внешних провайдеров (OAuth) · `superseded` (ADR-022)
  - [[ADR-022-standalone-terminal-pivot]] — **пивот на автономный встраиваемый терминал** · `accepted`
  - [[ADR-023-white-label-integration]] — интеграция как White Label-страница (бренд/SSO/события) · `accepted` (Т1 сделано)

### Сервисы и модули
- [[services-index]] — список всех сервисов, у каждого свой doc

### Эксплуатация проекта
- [[06-operations]] — репозиторий, CI/CD, запуск на новой машине, переменные окружения
- [[07-crm-onboarding]] — брифинг для сессии, которая пишет CRM/бэк-офис
- [[bug-log]] — журнал багов (симптом → причина → фикс → регресс-тест)
- [[backlog]] — идеи и дальнейшие улучшения

## 📌 Статус проекта

| | |
|---|---|
| Продукт | **Автономный терминал** (ADR-022): реальные цены Binance + бумажные деньги, без логина |
| Структура | `backend/`: `domain`+`broker`+`persistence`+`gateway` (→Binance) · `apps/terminal/`: nginx + JS |
| Запуск | `docker compose up --build` → **http://localhost:8888** (postgres + gateway + terminal) |
| Следующий шаг | Сайт платформы и CMS (отдельно) · интеграция терминала во внешние сайты · плечо/ликвидация |
| Дата | 2026-07-25 |

## 🗺️ Дорожная карта

1. **Ядро терминала** ✅ — данные Binance + бумажный брокер + durable-состояние (PostgreSQL)
2. **Пивот на автономный терминал** ✅ (2026-07-25, ADR-022) — снят мультипродуктовый контур
3. **Дальше** — сайт платформы и CMS (отдельные сущности); интеграция терминала во внешние сайты
   (передача личности пользователя, изоляция счетов); плечо и ликвидация в [[broker]]
