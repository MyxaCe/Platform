---
tags: [service, core]
status: active
updated: 2026-07-17
---

# Order Book

## Назначение

Стакан заявок с приоритетом **«цена, затем время»** (price-time priority). Хранит стоящие
заявки (makers) и умеет снимать встречную ликвидность для входящей заявки. Событий **не**
порождает — только меняет своё состояние и возвращает факты о сделках. Реализация: `core/src/book.rs`.

## Устройство

- `bids: BTreeMap<Price, VecDeque<RestingOrder>>` — покупки, лучшая цена = максимальная.
- `asks: BTreeMap<Price, VecDeque<RestingOrder>>` — продажи, лучшая цена = минимальная.
- `VecDeque` внутри ценового уровня → FIFO (приоритет по времени прихода).
- `locations: HashMap<OrderId, (Side, Price)>` → O(1) поиск при отмене.

## Публичный API

- `struct RestingOrder { id, qty, seq }` — заявка в стакане; `seq` = номер прихода (приоритет времени).
- `struct Fill { maker, price, qty, maker_fully_filled }` — факт исполнения против одного maker'а.
- `OrderBook::new()`
- `best_bid() -> Option<Price>` / `best_ask() -> Option<Price>`
- `contains(OrderId) -> bool`
- `qty_at(Side, Price) -> Qty` — суммарный объём на уровне (аналитика/тесты)
- `is_crossed() -> bool` — **инвариант**: `false` в норме (bid < ask)
- `insert(Side, Price, RestingOrder)` — поставить maker'а
- `cancel(OrderId) -> Option<Qty>` — снять; `None`, если заявки нет
- `cross(taker_side, limit: Option<Price>, qty) -> (Vec<Fill>, remaining: Qty)` ⭐ — снять ликвидность
  встречной стороны; `limit=None` для рыночной. Обход строго по приоритету цена→время → детерминизм.

## Связи

- Использует [[domain-types]] (`Price`, `Qty`, `Side`, `OrderId`).
- Вызывается из [[matching-engine]] (движок превращает `Fill` в события `Trade`).
- Одна книга = один инструмент; движок хранит `HashMap<InstrumentId, OrderBook>` ([[instrument-registry]], ADR-005).

## Инварианты

- После любой операции `is_crossed() == false` (проверяется тестом `book_never_stays_crossed_after_matching`).
- `locations` согласован с содержимым `bids`/`asks` (добитые maker'ы удаляются из индекса).

## Ограничения / TODO

- `cancel` линейно ищет внутри ценового уровня (`position`) — приемлемо; при больших уровнях
  можно оптимизировать. См. [[backlog]].
