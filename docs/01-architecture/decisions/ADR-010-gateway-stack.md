---
tags: [adr]
status: accepted
updated: 2026-07-17
---

# ADR-010 — Стек сетевого шлюза (Order Gateway)

## Статус

`accepted`.

## Контекст

Оркестратор ([[orchestrator]]) — библиотека; её нужно выставить наружу как сервис (REST для команд/
чтения + WS для живой ленты событий), чтобы биржу можно было дёргать по сети и позже подключить UI.
Это первый крейт с внешними зависимостями (за пределами `std`).

## Решение

- **Async-рантайм:** `tokio`.
- **HTTP/WS:** `axum` (0.7) поверх hyper/tokio.
- **Сериализация:** `serde` + `serde_json`. **Доменные/ядровые типы остаются чистыми** — DTO
  (request/response) живут в крейте `gateway` (граница = адаптер, hexagonal). Конвертация `Event → DTO`.
- **Конкурентность:** оркестратор — единственный писатель (наш принцип single-writer). Держим его за
  `tokio::sync::Mutex`; каждый запрос коротко берёт лок, выполняет операцию, отпускает. Порядок входа
  = порядок захвата лока. Позже можно заменить на actor/командный канал с выделенным потоком.
- **Auth (dev-уровень):** Bearer-токен → `UserId` (in-memory реестр). **Не продакшн**: нет хэшей,
  сессий, ротации. Настоящий auth (пароли/2FA/JWT/KYC) — отдельная фаза. См. [[backlog]].
- **Живая лента:** `tokio::sync::broadcast`; после каждой операции публикуем события (JSON), WS-клиенты
  на `/stream` их получают.

## API (первый срез)

```
GET  /health
POST /admin/users            {token}                       → {user_id}     (dev)
POST /admin/instruments      {id, symbol, base, quote, …}                  (dev)
POST /admin/deposit          {user_id, asset, amount}                       (dev)
POST /orders                 (Bearer) {instrument, side, type, price?, qty, tif?} → {order_id, events}
DELETE /orders/{id}?instrument=N  (Bearer)
GET  /book/{instrument}?depth=N   → {bids, asks}
GET  /balance/{asset}        (Bearer) → {available, held}
WS   /stream                 живая лента событий
```

## Последствия

- Docker-сборка теперь тянет crates.io (network at build); тесты кэшируют target/registry в volume'ах.
- Первый живой сервис в Docker (`docker compose up`).
- **Суммы в JSON — числами** (первый срез). Для JS-клиентов большие `i128` теряют точность → позже
  кодировать суммы строками. См. [[backlog]].
- Admin-эндпоинты пока без защиты (dev). Прод — за admin-ключом/ролью. [[backlog]].
