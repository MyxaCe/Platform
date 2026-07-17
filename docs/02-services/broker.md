---
tags: [service, broker]
status: active
updated: 2026-07-17
---

# Broker (бумажный)

## Назначение

Исполнение сделок в broker-режиме на **бумажных деньгах** и **реальных ценах** (ADR-014):
открыл BUY/SELL позицию по рынку → P&L двигается по реальной цене → закрыл, зафиксировал прибыль/
убыток. Реальных денег нет, депозиты виртуальные. Крейт `broker/` (чистая логика, зависит от `domain`).

## Модель

- Деньги — в **центах** (`i128`). Счёт: стартовый баланс (в gateway — $100k).
- Позиция: `{ instrument, side: Long|Short, qty(raw), entry(raw), pd, qd, margin }`.
- `value = price_raw · qty_raw · 100 / 10^(pd+qd)` (центы). PnL Long = `value(mark−entry)`, Short = `value(entry−mark)`.
- Открытие: маржа = нотионал / leverage (пока **leverage=1**); нужно `free = balance − used_margin ≥ margin`.
- Закрытие: `balance += realized_pnl`. Equity = `balance + Σ нереализованный PnL`.

## Публичный API

- `Broker::new(start_balance, leverage)`
- `open(user, instrument, side, qty, entry, pd, qd) -> Result<pos_id, BrokerError>`
- `close(user, id, mark) -> Result<Cents>` (реализованный P&L)
- Чтение: `balance/used_margin/free_margin/positions/open_pnl/equity`
- `enum BrokerError { InsufficientMargin, UnknownPosition }`

## Связи

- Используется [[gateway]]: эндпоинты `POST /deals`, `GET /deals`, `POST /deals/{id}/close`, `GET /account`.
- Цены входа/марк — из реального фида ([[gateway]] `feed`): buy=ask, sell=bid, марк=last.
- Основано на [[ADR-014-paper-broker]].

## Тесты

`broker/tests/broker.rs` — 8 тестов: маржа/free, отказ по марже, P&L long/short, закрытие в плюс/минус,
неизвестная позиция, освобождение маржи.

## Ограничения / TODO

- **leverage=1**, нет ликвидации/margin-call, стоп/тейк, комиссий, свопов — [[backlog]].
- Позиции **в памяти** (не персистентны) — durable-хранилище позже.
