//! Order book — стакан заявок с приоритетом «цена, затем время» (price-time priority).
//!
//! Устройство:
//! - Каждая сторона — `BTreeMap<Price, VecDeque<RestingOrder>>`: цена → FIFO-очередь заявок.
//!   `BTreeMap` держит цены отсортированными (лучшая покупка = максимальная, лучшая продажа =
//!   минимальная), `VecDeque` внутри уровня даёт приоритет по времени прихода.
//! - `locations`: индекс `OrderId -> (Side, Price)` для O(1) поиска при отмене.
//!
//! Матчинг встречной стороны инкапсулирован в [`OrderBook::cross`]. Сам book не порождает
//! события и не знает про них — он лишь меняет состояние стакана и возвращает факты о сделках
//! ([`Fill`]). Превращение фактов в события — ответственность [`crate::engine`].

use std::collections::{BTreeMap, HashMap, VecDeque};

use domain::money::{Price, Qty};
use domain::order::{OrderId, Side};

/// Заявка, стоящая в стакане (maker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestingOrder {
    pub id: OrderId,
    pub qty: Qty,
    /// Порядковый номер прихода — для приоритета по времени (детерминирован, не из часов).
    pub seq: u64,
}

/// Факт исполнения против одной стоявшей в стакане заявки. Возвращается из [`OrderBook::cross`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fill {
    pub maker: OrderId,
    pub price: Price,
    pub qty: Qty,
    /// Была ли заявка-maker исполнена полностью (и удалена из стакана).
    pub maker_fully_filled: bool,
}

/// Один агрегированный ценовой уровень в снапшоте стакана.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Level {
    pub price: Price,
    /// Суммарный объём всех заявок на этом уровне.
    pub qty: Qty,
    /// Число заявок на уровне.
    pub orders: u32,
}

/// Снапшот глубины стакана (read-model): агрегированные уровни bid/ask.
/// `bids` — по убыванию цены, `asks` — по возрастанию (лучшая цена в начале каждого списка).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepthSnapshot {
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
}

#[derive(Debug, Default)]
pub struct OrderBook {
    /// Покупки. Лучшая цена — максимальная (`keys().next_back()`).
    bids: BTreeMap<Price, VecDeque<RestingOrder>>,
    /// Продажи. Лучшая цена — минимальная (`keys().next()`).
    asks: BTreeMap<Price, VecDeque<RestingOrder>>,
    /// Индекс расположения заявок для быстрой отмены.
    locations: HashMap<OrderId, (Side, Price)>,
}

impl OrderBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Лучшая цена покупки (bid).
    pub fn best_bid(&self) -> Option<Price> {
        self.bids.keys().next_back().copied()
    }

    /// Лучшая цена продажи (ask).
    pub fn best_ask(&self) -> Option<Price> {
        self.asks.keys().next().copied()
    }

    /// Стоит ли заявка с таким id в стакане.
    pub fn contains(&self, id: OrderId) -> bool {
        self.locations.contains_key(&id)
    }

    /// Суммарный объём на заданном ценовом уровне указанной стороны (для тестов/аналитики).
    pub fn qty_at(&self, side: Side, price: Price) -> Qty {
        let map = match side {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        };
        map.get(&price)
            .map(|level| level.iter().fold(Qty::ZERO, |acc, o| acc + o.qty))
            .unwrap_or(Qty::ZERO)
    }

    /// Инвариант рынка: bid не должен быть ≥ ask (иначе книга «скрещена» — это баг матчинга).
    pub fn is_crossed(&self) -> bool {
        matches!((self.best_bid(), self.best_ask()), (Some(b), Some(a)) if b >= a)
    }

    /// Снапшот глубины: до `depth` лучших ценовых уровней с каждой стороны, с агрегированным
    /// объёмом и числом заявок. Read-only проекция для market data / визуализации.
    pub fn snapshot(&self, depth: usize) -> DepthSnapshot {
        fn aggregate((price, level): (&Price, &VecDeque<RestingOrder>)) -> Level {
            Level {
                price: *price,
                qty: level.iter().fold(Qty::ZERO, |acc, o| acc + o.qty),
                orders: level.len() as u32,
            }
        }
        // bids: BTreeMap по возрастанию → rev даёт лучшую (максимальную) цену первой.
        let bids = self.bids.iter().rev().take(depth).map(aggregate).collect();
        let asks = self.asks.iter().take(depth).map(aggregate).collect();
        DepthSnapshot { bids, asks }
    }

    /// Поставить заявку в стакан как maker.
    pub fn insert(&mut self, side: Side, price: Price, order: RestingOrder) {
        self.locations.insert(order.id, (side, price));
        let map = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        map.entry(price).or_default().push_back(order);
    }

    /// Снять заявку из стакана. Возвращает её оставшийся объём, либо `None`, если её там нет.
    pub fn cancel(&mut self, id: OrderId) -> Option<Qty> {
        let (side, price) = self.locations.remove(&id)?;
        let map = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        let level = map.get_mut(&price)?;
        let pos = level.iter().position(|o| o.id == id)?;
        let removed = level.remove(pos)?;
        if level.is_empty() {
            map.remove(&price);
        }
        Some(removed.qty)
    }

    /// Снять ликвидность встречной стороны для входящей заявки (taker).
    ///
    /// - `taker_side` — сторона входящей заявки; матчимся против противоположной стороны.
    /// - `limit` — предельная цена для лимитной заявки; `None` для рыночной (любая цена).
    /// - `qty` — объём, который хочет исполнить taker.
    ///
    /// Возвращает список исполнений и **неисполненный остаток**. Порядок обхода —
    /// строго по приоритету «лучшая цена → раньше пришедший», что гарантирует детерминизм.
    pub fn cross(&mut self, taker_side: Side, limit: Option<Price>, mut qty: Qty) -> (Vec<Fill>, Qty) {
        let mut fills: Vec<Fill> = Vec::new();
        let mut emptied: Vec<OrderId> = Vec::new();

        while qty.is_positive() {
            // Лучшая цена встречной стороны.
            let best = match taker_side {
                Side::Buy => self.asks.keys().next().copied(),
                Side::Sell => self.bids.keys().next_back().copied(),
            };
            let Some(price) = best else { break };

            // Для лимитной заявки — проверка, что цена «скрещивается».
            if let Some(lim) = limit {
                let crosses = match taker_side {
                    Side::Buy => price <= lim,
                    Side::Sell => price >= lim,
                };
                if !crosses {
                    break;
                }
            }

            let opposite = match taker_side {
                Side::Buy => &mut self.asks,
                Side::Sell => &mut self.bids,
            };
            let level = opposite
                .get_mut(&price)
                .expect("уровень существует для лучшей цены");

            // Матчинг внутри уровня по FIFO (приоритет по времени).
            while qty.is_positive() {
                let (maker_id, maker_qty) = match level.front() {
                    Some(o) => (o.id, o.qty),
                    None => break,
                };
                let traded = qty.min(maker_qty);
                let maker_left = maker_qty - traded;
                qty = qty - traded;

                let maker_fully_filled = maker_left.is_zero();
                fills.push(Fill {
                    maker: maker_id,
                    price,
                    qty: traded,
                    maker_fully_filled,
                });

                if maker_fully_filled {
                    level.pop_front();
                    emptied.push(maker_id);
                } else {
                    // maker исполнен частично → taker точно исчерпан, обновляем остаток maker'а.
                    level.front_mut().expect("front есть").qty = maker_left;
                }
            }

            if level.is_empty() {
                opposite.remove(&price);
            }
        }

        // Индекс расположения чистим после того, как отпустили заимствование bids/asks.
        for id in emptied {
            self.locations.remove(&id);
        }

        (fills, qty)
    }
}
