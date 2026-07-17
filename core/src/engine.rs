//! Matching Engine — сердце ядра (Фаза 1).
//!
//! Единственная точка входа — [`MatchingEngine::apply`]: `(state, command) -> events`.
//! Движок детерминирован: никакого времени/случайности/I-O. Держит по одной книге на инструмент
//! (ADR-005) и маршрутизирует команды по `instrument`. Порядок приоритета в стакане — по
//! возрастающему `seq` (номер прихода), который движок выдаёт сам из своего состояния.
//!
//! Дисциплина событий на размещение заявки:
//! 1. `OrderAccepted` (после валидации),
//! 2. по одному `Trade` на каждое исполнение (+ `OrderFilled` для полностью исполненного maker'а),
//! 3. финальная судьба taker'а: `OrderFilled` | `OrderResting` | `OrderCanceledRemainder`.

use std::collections::HashMap;

use crate::book::{DepthSnapshot, OrderBook, RestingOrder};
use crate::command::Command;
use crate::event::{Event, RejectReason, TradeId};
use domain::instrument::{Instrument, InstrumentId, InstrumentRegistry};
use domain::money::{Price, Qty};
use domain::order::{OrderId, OrderType, Side, TimeInForce};

#[derive(Debug, Default)]
pub struct MatchingEngine {
    instruments: InstrumentRegistry,
    /// По одной книге на инструмент.
    books: HashMap<InstrumentId, OrderBook>,
    /// Монотонный счётчик приоритета по времени (номер прихода заявки), глобальный.
    seq: u64,
    /// Монотонный счётчик id сделок, глобальный.
    next_trade_id: u64,
}

impl MatchingEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Зарегистрировать инструмент и создать для него пустую книгу. Делается до торгов.
    pub fn register_instrument(&mut self, instrument: Instrument) {
        let id = instrument.id;
        self.instruments.register(instrument);
        self.books.entry(id).or_default();
    }

    /// Только для чтения: книга инструмента (тесты, снапшоты, будущие проекции).
    pub fn book(&self, instrument: InstrumentId) -> Option<&OrderBook> {
        self.books.get(&instrument)
    }

    /// Снапшот глубины стакана инструмента (до `depth` уровней с каждой стороны).
    /// `None`, если инструмент не зарегистрирован.
    pub fn snapshot(&self, instrument: InstrumentId, depth: usize) -> Option<DepthSnapshot> {
        self.books.get(&instrument).map(|b| b.snapshot(depth))
    }

    /// Применить команду. Чистое по духу преобразование состояния: возвращает список событий.
    pub fn apply(&mut self, cmd: Command) -> Vec<Event> {
        match cmd {
            Command::PlaceOrder { instrument, id, side, order_type, qty, tif } => {
                self.place(instrument, id, side, order_type, qty, tif)
            }
            Command::CancelOrder { instrument, id } => self.cancel(instrument, id),
        }
    }

    fn place(
        &mut self,
        instrument: InstrumentId,
        id: OrderId,
        side: Side,
        order_type: OrderType,
        qty: Qty,
        tif: TimeInForce,
    ) -> Vec<Event> {
        // --- Валидация против параметров инструмента (ADR-005) ---
        let reject = |reason| vec![Event::OrderRejected { instrument, id, reason }];

        let inst = match self.instruments.get(instrument) {
            Some(i) => i,
            None => return reject(RejectReason::UnknownInstrument),
        };
        // Копируем скалярные параметры, чтобы отпустить заимствование реестра.
        let tick = inst.tick_size;
        let lot = inst.lot_size;
        let min_qty = inst.min_qty;

        if !qty.is_positive() {
            return reject(RejectReason::NonPositiveQty);
        }
        if qty.0 % lot.0 != 0 {
            return reject(RejectReason::QtyNotOnLot);
        }
        if qty < min_qty {
            return reject(RejectReason::BelowMinQty);
        }
        if let OrderType::Limit { price } = order_type {
            if price.0 <= 0 {
                return reject(RejectReason::NonPositivePrice);
            }
            if price.0 % tick.0 != 0 {
                return reject(RejectReason::PriceNotOnTick);
            }
        }

        // --- Матчинг ---
        let seq = self.seq;
        self.seq += 1;

        let limit: Option<Price> = match order_type {
            OrderType::Limit { price } => Some(price),
            OrderType::Market => None,
        };

        // Все мутации книги — здесь; заимствование `book` заканчивается до сборки событий,
        // чтобы не конфликтовать с self.next_trade_id.
        let book = self.books.get_mut(&instrument).expect("книга есть для зарегистрированного инструмента");
        let (fills, remaining) = book.cross(side, limit, qty);

        let rested: Option<(Price, Qty)> = if remaining.is_positive() {
            let is_market = matches!(order_type, OrderType::Market);
            let rest_in_book = matches!(tif, TimeInForce::Gtc) && !is_market;
            match (rest_in_book, order_type) {
                (true, OrderType::Limit { price }) => {
                    book.insert(side, price, RestingOrder { id, qty: remaining, seq });
                    Some((price, remaining))
                }
                _ => None,
            }
        } else {
            None
        };
        // (заимствование `book` больше не используется ниже)

        // --- Сборка событий ---
        let mut events = Vec::with_capacity(fills.len() + 2);
        events.push(Event::OrderAccepted { instrument, id });

        for f in &fills {
            let trade_id = TradeId(self.next_trade_id);
            self.next_trade_id += 1;
            events.push(Event::Trade {
                instrument,
                id: trade_id,
                price: f.price,
                qty: f.qty,
                taker: id,
                maker: f.maker,
                taker_side: side,
            });
            if f.maker_fully_filled {
                events.push(Event::OrderFilled { instrument, id: f.maker });
            }
        }

        if remaining.is_zero() {
            events.push(Event::OrderFilled { instrument, id });
        } else if let Some((price, qty)) = rested {
            events.push(Event::OrderResting { instrument, id, price, qty });
        } else {
            events.push(Event::OrderCanceledRemainder { instrument, id, qty: remaining });
        }

        events
    }

    fn cancel(&mut self, instrument: InstrumentId, id: OrderId) -> Vec<Event> {
        let book = match self.books.get_mut(&instrument) {
            Some(b) => b,
            None => {
                return vec![Event::OrderRejected {
                    instrument,
                    id,
                    reason: RejectReason::UnknownInstrument,
                }]
            }
        };
        match book.cancel(id) {
            Some(_qty) => vec![Event::OrderCanceled { instrument, id }],
            None => vec![Event::OrderRejected {
                instrument,
                id,
                reason: RejectReason::UnknownOrder,
            }],
        }
    }
}
