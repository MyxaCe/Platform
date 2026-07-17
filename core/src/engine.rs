//! Matching Engine — сердце ядра (Фаза 1).
//!
//! Единственная точка входа — [`MatchingEngine::apply`]: `(state, command) -> events`.
//! Движок детерминирован: никакого времени/случайности/I-O. Порядок приоритета в стакане —
//! по возрастающему `seq` (номер прихода), который движок выдаёт сам из своего состояния.
//!
//! Дисциплина событий на размещение заявки:
//! 1. `OrderAccepted` (после валидации),
//! 2. по одному `Trade` на каждое исполнение (+ `OrderFilled` для полностью исполненного maker'а),
//! 3. финальная судьба taker'а: `OrderFilled` | `OrderResting` | `OrderCanceledRemainder`.

use crate::book::{OrderBook, RestingOrder};
use crate::command::Command;
use crate::domain::money::Qty;
use crate::domain::order::{OrderId, OrderType, Side, TimeInForce};
use crate::event::{Event, RejectReason, TradeId};

#[derive(Debug, Default)]
pub struct MatchingEngine {
    book: OrderBook,
    /// Монотонный счётчик приоритета по времени (номер прихода заявки).
    seq: u64,
    /// Монотонный счётчик id сделок.
    next_trade_id: u64,
}

impl MatchingEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Только для чтения: доступ к стакану (тесты, снапшоты, будущие проекции).
    pub fn book(&self) -> &OrderBook {
        &self.book
    }

    /// Применить команду. Чистое по духу преобразование состояния: возвращает список событий.
    pub fn apply(&mut self, cmd: Command) -> Vec<Event> {
        match cmd {
            Command::PlaceOrder {
                id,
                side,
                order_type,
                qty,
                tif,
            } => self.place(id, side, order_type, qty, tif),
            Command::CancelOrder { id } => self.cancel(id),
        }
    }

    fn place(
        &mut self,
        id: OrderId,
        side: Side,
        order_type: OrderType,
        qty: Qty,
        tif: TimeInForce,
    ) -> Vec<Event> {
        let mut events = Vec::new();

        // Валидация.
        if !qty.is_positive() {
            events.push(Event::OrderRejected {
                id,
                reason: RejectReason::NonPositiveQty,
            });
            return events;
        }
        events.push(Event::OrderAccepted { id });

        // Номер прихода для приоритета по времени.
        let seq = self.seq;
        self.seq += 1;

        // Матчинг против встречной стороны.
        let limit = match order_type {
            OrderType::Limit { price } => Some(price),
            OrderType::Market => None,
        };
        let (fills, remaining) = self.book.cross(side, limit, qty);

        // Факты исполнения → события Trade (+ OrderFilled для добитых maker'ов).
        for f in &fills {
            let trade_id = TradeId(self.next_trade_id);
            self.next_trade_id += 1;
            events.push(Event::Trade {
                id: trade_id,
                price: f.price,
                qty: f.qty,
                taker: id,
                maker: f.maker,
                taker_side: side,
            });
            if f.maker_fully_filled {
                events.push(Event::OrderFilled { id: f.maker });
            }
        }

        // Судьба входящей заявки (taker).
        if remaining.is_zero() {
            events.push(Event::OrderFilled { id });
        } else {
            let is_market = matches!(order_type, OrderType::Market);
            let rest_in_book = matches!(tif, TimeInForce::Gtc) && !is_market;

            match (rest_in_book, order_type) {
                (true, OrderType::Limit { price }) => {
                    self.book.insert(side, price, RestingOrder { id, qty: remaining, seq });
                    events.push(Event::OrderResting { id, price, qty: remaining });
                }
                // Рыночная, либо IOC-лимитная: остаток не встаёт в стакан.
                _ => {
                    events.push(Event::OrderCanceledRemainder { id, qty: remaining });
                }
            }
        }

        events
    }

    fn cancel(&mut self, id: OrderId) -> Vec<Event> {
        match self.book.cancel(id) {
            Some(_qty) => vec![Event::OrderCanceled { id }],
            None => vec![Event::OrderRejected {
                id,
                reason: RejectReason::UnknownOrder,
            }],
        }
    }
}
