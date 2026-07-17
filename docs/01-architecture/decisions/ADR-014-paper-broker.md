---
tags: [adr]
status: accepted
updated: 2026-07-17
---

# ADR-014 — Бумажный брокер (позиции, маржа, P&L)

## Статус

`accepted`.

## Контекст

Терминал (ADR-013) показывает реальные цены, но сделки не исполняются. Нужно **исполнение
broker-режима на бумажных деньгах**: открыл BUY/SELL по рыночной цене → P&L двигается по реальной
цене → закрыл, зафиксировал. **Реальных денег нет**, депозиты виртуальные; можно и выиграть, и
проиграть.

## Решение

Отдельный крейт **`broker`** (чистая логика, зависит от `domain`):

- **Счёт**: `balance` (реализованные бумажные деньги, в **центах** `i128`), стартовый депозит (напр.
  $100k). Позиции по id.
- **Позиция**: `{ instrument, side: Long|Short, qty(raw), entry(raw price), pd, qd, margin }`.
- **Стоимость/PnL в центах**: `value = price_raw · qty_raw · 100 / 10^(pd+qd)`. Нереализованный PnL
  для Long = `value(mark − entry)`, для Short = `value(entry − mark)`.
- **Открытие**: маржа = нотионал / leverage (пока **leverage = 1**, маржа = полный нотионал → убыток
  ограничен нотионалом, счёт не уходит в глубокий минус). Требуется `free = balance − used_margin ≥ маржа`.
- **Закрытие**: `balance += realized_pnl`, позиция удаляется.
- **Equity** = `balance + Σ нереализованный PnL`; **free margin** = `equity − used_margin`.

**Цены** берём из реального фида (ADR-013): вход buy=ask/sell=bid; марк-цена для PnL/закрытия = last.

## Gateway

Эндпоинты (Bearer): `POST /deals` (открыть), `GET /deals` (позиции + live PnL), `POST /deals/{id}/close`,
`GET /account` (balance/equity/margin/free/open_pnl). Матчинг-путь (`/orders`) остаётся отдельно (фон).

## UI

BUY/SELL → `/deals`; нижняя вкладка **OPEN DEALS** с позициями и кнопкой Close; верхние метрики
(BALANCE/EQUITY/MARGIN/FREE/OPEN P&L) — из `/account`.

## Последствия

- Только бумажные деньги; **никакого реального custody**. Депозиты — виртуальные.
- Leverage=1, нет ликвидации/margin-call (в [[backlog]]). Позже — плечо, стоп/тейк, комиссии, свопы.
- Спред учитывается (вход по ask/bid, марк по last).
