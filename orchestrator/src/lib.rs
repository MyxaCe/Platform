//! # orchestrator
//!
//! Связывает [`ledger`] (деньги) и [`exchange_core`] (матчинг) в полный путь заявки
//! по схеме **reserve-before-match, settle-from-events** (ADR-008):
//! резерв средств → матчинг → расчёт сделок / возврат резерва по событиям движка.
//!
//! Точки входа: [`Orchestrator::place_limit`] и [`Orchestrator::cancel`].

use std::collections::HashMap;

use domain::account::UserId;
use domain::instrument::{AssetId, Instrument, InstrumentId};
use domain::money::{notional, Amount, Price, Qty};
use domain::order::{OrderId, OrderType, Side, TimeInForce};

use exchange_core::{Command, DepthSnapshot, Event, MatchingEngine, OrderBook, RejectReason};
use ledger::{Balance, Ledger};

/// Причина отказа на уровне оркестратора.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderReject {
    /// Недостаточно свободных средств для резерва.
    InsufficientFunds,
    /// Отказ движка (валидация): инструмент/tick/lot/min и т.п.
    Engine(RejectReason),
    /// Попытка отменить чужую заявку.
    NotOwner,
}

/// Запись о живой заявке: связывает заявку движка с пользователем и его резервом.
///
/// Цену резерва хранить не нужно: для taker'а она известна из входящей заявки, а для maker'а
/// равна цене сделки (сделки идут по цене maker'а) — см. ADR-008.
#[derive(Debug, Clone)]
struct OrderRecord {
    user: UserId,
    reserved_asset: AssetId,
    /// Ещё не израсходованный (не рассчитанный/не возвращённый) резерв этой заявки.
    reserved_remaining: Amount,
}

/// Оркестратор владеет движком, ledger'ом и реестром живых заявок.
#[derive(Debug, Default)]
pub struct Orchestrator {
    engine: MatchingEngine,
    ledger: Ledger,
    orders: HashMap<OrderId, OrderRecord>,
}

impl Orchestrator {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- Настройка / чтение ----------------------------------------------

    pub fn register_instrument(&mut self, inst: Instrument) {
        self.engine.register_instrument(inst);
    }

    pub fn deposit(&mut self, user: UserId, asset: AssetId, amount: Amount) {
        self.ledger.deposit(user, asset, amount);
    }

    pub fn balance(&self, user: UserId, asset: AssetId) -> Balance {
        self.ledger.balance(user, asset)
    }
    pub fn available(&self, user: UserId, asset: AssetId) -> Amount {
        self.ledger.available(user, asset)
    }
    pub fn held(&self, user: UserId, asset: AssetId) -> Amount {
        self.ledger.held(user, asset)
    }
    pub fn total_supply(&self, asset: AssetId) -> Amount {
        self.ledger.total_supply(asset)
    }
    pub fn book(&self, instrument: InstrumentId) -> Option<&OrderBook> {
        self.engine.book(instrument)
    }
    pub fn snapshot(&self, instrument: InstrumentId, depth: usize) -> Option<DepthSnapshot> {
        self.engine.snapshot(instrument, depth)
    }

    // ---- Путь ордера ------------------------------------------------------

    /// Разместить лимитную заявку: резерв → матчинг → расчёт.
    #[allow(clippy::too_many_arguments)]
    pub fn place_limit(
        &mut self,
        user: UserId,
        instrument: InstrumentId,
        id: OrderId,
        side: Side,
        price: Price,
        qty: Qty,
        tif: TimeInForce,
    ) -> Result<Vec<Event>, OrderReject> {
        // 1. Инструмент (нужны активы base/quote для резерва).
        let (base, quote) = match self.engine.instrument(instrument) {
            Some(i) => (i.base, i.quote),
            None => return Err(OrderReject::Engine(RejectReason::UnknownInstrument)),
        };

        // 2. Резерв: buy → quote (цена×объём), sell → base (объём).
        let (reserved_asset, reserved_amount) = match side {
            Side::Buy => (quote, notional(price, qty)),
            Side::Sell => (base, Amount(qty.0 as i128)),
        };
        if self.ledger.reserve(user, reserved_asset, reserved_amount).is_err() {
            return Err(OrderReject::InsufficientFunds);
        }
        self.orders.insert(
            id,
            OrderRecord { user, reserved_asset, reserved_remaining: reserved_amount },
        );

        // 3. Матчинг.
        let events = self.engine.apply(Command::PlaceOrder {
            instrument,
            id,
            side,
            order_type: OrderType::Limit { price },
            qty,
            tif,
        });

        // 4. Отказ движка → вернуть резерв целиком.
        if let Some(reason) = rejected_reason(&events, id) {
            self.release_and_forget(id);
            return Err(OrderReject::Engine(reason));
        }

        // 5. Расчёты по событиям.
        self.settle_events(base, quote, price, &events);
        Ok(events)
    }

    /// Отменить свою заявку и вернуть её резерв.
    pub fn cancel(
        &mut self,
        user: UserId,
        instrument: InstrumentId,
        id: OrderId,
    ) -> Result<Vec<Event>, OrderReject> {
        if let Some(rec) = self.orders.get(&id) {
            if rec.user != user {
                return Err(OrderReject::NotOwner);
            }
        }
        let events = self.engine.apply(Command::CancelOrder { instrument, id });
        if events.iter().any(|e| matches!(e, Event::OrderCanceled { .. })) {
            self.release_and_forget(id);
            Ok(events)
        } else {
            Err(OrderReject::Engine(RejectReason::UnknownOrder))
        }
    }

    // ---- Внутреннее -------------------------------------------------------

    /// Применить расчёты ledger'а по событиям движка (входящая заявка — `taker`, цена `taker_price`).
    fn settle_events(&mut self, base: AssetId, quote: AssetId, taker_price: Price, events: &[Event]) {
        for e in events {
            match e {
                Event::Trade { price, qty, taker, maker, taker_side, .. } => {
                    // Определяем покупателя/продавца и цену резерва покупателя.
                    let (buyer_id, seller_id, buyer_reserve_price) = match taker_side {
                        Side::Buy => (*taker, *maker, taker_price),
                        Side::Sell => (*maker, *taker, *price),
                    };
                    let buyer_user = self.orders.get(&buyer_id).expect("покупатель записан").user;
                    let seller_user = self.orders.get(&seller_id).expect("продавец записан").user;

                    self.ledger
                        .settle_fill(base, quote, buyer_user, seller_user, *price, *qty, buyer_reserve_price)
                        .expect("резерв обеспечен оркестратором");

                    // Уменьшаем остаток резерва участников на израсходованное.
                    let buyer_spent = Amount(buyer_reserve_price.0 as i128 * qty.0 as i128);
                    let seller_spent = Amount(qty.0 as i128);
                    if let Some(b) = self.orders.get_mut(&buyer_id) {
                        b.reserved_remaining = b.reserved_remaining - buyer_spent;
                    }
                    if let Some(s) = self.orders.get_mut(&seller_id) {
                        s.reserved_remaining = s.reserved_remaining - seller_spent;
                    }
                }
                Event::OrderFilled { id, .. } => {
                    // Полностью исполнена — резерв израсходован, запись больше не нужна.
                    self.orders.remove(id);
                }
                Event::OrderCanceledRemainder { id, .. } => {
                    // IOC/рыночный остаток — вернуть неизрасходованный резерв.
                    self.release_and_forget(*id);
                }
                _ => {}
            }
        }
    }

    /// Вернуть неизрасходованный резерв заявки в `available` и забыть её.
    fn release_and_forget(&mut self, id: OrderId) {
        if let Some(rec) = self.orders.remove(&id) {
            if rec.reserved_remaining.is_positive() {
                self.ledger
                    .release(rec.user, rec.reserved_asset, rec.reserved_remaining)
                    .expect("held покрывает остаток резерва");
            }
        }
    }
}

/// Причина отказа движка для конкретной заявки, если она есть в событиях.
fn rejected_reason(events: &[Event], id: OrderId) -> Option<RejectReason> {
    events.iter().find_map(|e| match e {
        Event::OrderRejected { id: rid, reason, .. } if *rid == id => Some(*reason),
        _ => None,
    })
}
