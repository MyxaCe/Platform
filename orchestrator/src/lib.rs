//! # orchestrator
//!
//! Связывает [`ledger`] (деньги) и [`exchange_core`] (матчинг) в полный путь заявки по схеме
//! **reserve-before-match, settle-from-events** (ADR-008): резерв средств → матчинг → расчёт
//! сделок / возврат резерва по событиям движка.
//!
//! Поддержаны лимитные и рыночные заявки; есть предотвращение self-trade (ADR-009).
//! Точки входа: [`Orchestrator::place_limit`], [`Orchestrator::place_market`], [`Orchestrator::cancel`].

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
    /// Заявка пересеклась бы со своей же встречной (ADR-009).
    SelfTrade,
}

/// Запись о живой заявке: связывает заявку движка с пользователем и его резервом.
///
/// В реестре лежат ровно стоящие в стакане заявки (исполненные/снятые удаляются). `side`/`price`
/// нужны для STP-проверки; для рыночных заявок запись живёт один такт и не сканируется.
#[derive(Debug, Clone)]
struct OrderRecord {
    user: UserId,
    instrument: InstrumentId,
    side: Side,
    price: Price,
    reserved_asset: AssetId,
    /// Ещё не израсходованный (не рассчитанный/не возвращённый) резерв заявки.
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

    /// Разместить лимитную заявку.
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
        self.place(user, instrument, id, side, OrderType::Limit { price }, qty, tif)
    }

    /// Разместить рыночную заявку (по смыслу всегда IOC — остаток не встаёт в стакан).
    pub fn place_market(
        &mut self,
        user: UserId,
        instrument: InstrumentId,
        id: OrderId,
        side: Side,
        qty: Qty,
    ) -> Result<Vec<Event>, OrderReject> {
        self.place(user, instrument, id, side, OrderType::Market, qty, TimeInForce::Ioc)
    }

    #[allow(clippy::too_many_arguments)]
    fn place(
        &mut self,
        user: UserId,
        instrument: InstrumentId,
        id: OrderId,
        side: Side,
        order_type: OrderType,
        qty: Qty,
        tif: TimeInForce,
    ) -> Result<Vec<Event>, OrderReject> {
        // Инструмент (нужны активы base/quote для резерва).
        let (base, quote) = match self.engine.instrument(instrument) {
            Some(i) => (i.base, i.quote),
            None => return Err(OrderReject::Engine(RejectReason::UnknownInstrument)),
        };
        let limit: Option<Price> = match order_type {
            OrderType::Limit { price } => Some(price),
            OrderType::Market => None,
        };

        // Self-trade prevention: отклоняем без мутаций (ADR-009).
        if self.would_self_trade(user, instrument, side, limit) {
            return Err(OrderReject::SelfTrade);
        }

        // Резерв: buy-limit → quote (цена×объём), sell → base (объём),
        // buy-market → стоимость по стакану (своих асков нет благодаря STP).
        let (reserved_asset, reserved_amount) = match (side, order_type) {
            (Side::Buy, OrderType::Limit { price }) => (quote, notional(price, qty)),
            (Side::Buy, OrderType::Market) => {
                let (cost, _fillable) = self.market_buy_cost(instrument, qty);
                if cost > self.ledger.available(user, quote) {
                    return Err(OrderReject::InsufficientFunds);
                }
                (quote, cost)
            }
            (Side::Sell, _) => (base, Amount(qty.0 as i128)),
        };
        if self.ledger.reserve(user, reserved_asset, reserved_amount).is_err() {
            return Err(OrderReject::InsufficientFunds);
        }
        self.orders.insert(
            id,
            OrderRecord {
                user,
                instrument,
                side,
                price: limit.unwrap_or(Price(0)),
                reserved_asset,
                reserved_remaining: reserved_amount,
            },
        );

        // Матчинг.
        let events = self.engine.apply(Command::PlaceOrder { instrument, id, side, order_type, qty, tif });

        // Отказ движка → вернуть резерв целиком.
        if let Some(reason) = rejected_reason(&events, id) {
            self.release_and_forget(id);
            return Err(OrderReject::Engine(reason));
        }

        // Расчёты по событиям.
        self.settle_events(base, quote, limit, &events);
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

    /// Пересеклась бы заявка со своей же встречной стоящей заявкой?
    fn would_self_trade(&self, user: UserId, instrument: InstrumentId, side: Side, limit: Option<Price>) -> bool {
        self.orders.values().any(|r| {
            r.user == user
                && r.instrument == instrument
                && r.side == side.opposite()
                && crosses(side, limit, r.price)
        })
    }

    /// Стоимость и исполнимый объём рыночной покупки `qty` по текущим ask-уровням.
    /// Точно (книга не меняется между расчётом и матчингом; своих асков нет — STP выше).
    fn market_buy_cost(&self, instrument: InstrumentId, qty: Qty) -> (Amount, Qty) {
        let mut remaining = qty;
        let mut cost: i128 = 0;
        if let Some(snap) = self.engine.snapshot(instrument, usize::MAX) {
            for lvl in snap.asks {
                if !remaining.is_positive() {
                    break;
                }
                let take = remaining.min(lvl.qty);
                cost += lvl.price.0 as i128 * take.0 as i128;
                remaining = remaining - take;
            }
        }
        (Amount(cost), qty - remaining)
    }

    /// Применить расчёты ledger'а по событиям движка. `taker_limit` — лимит входящей заявки
    /// (`None` для рыночной; тогда цена резерва покупателя = цена сделки).
    fn settle_events(&mut self, base: AssetId, quote: AssetId, taker_limit: Option<Price>, events: &[Event]) {
        for e in events {
            match e {
                Event::Trade { price, qty, taker, maker, taker_side, .. } => {
                    let (buyer_id, seller_id, buyer_reserve_price) = match taker_side {
                        Side::Buy => (*taker, *maker, taker_limit.unwrap_or(*price)),
                        Side::Sell => (*maker, *taker, *price),
                    };
                    let buyer_user = self.orders.get(&buyer_id).expect("покупатель записан").user;
                    let seller_user = self.orders.get(&seller_id).expect("продавец записан").user;

                    self.ledger
                        .settle_fill(base, quote, buyer_user, seller_user, *price, *qty, buyer_reserve_price)
                        .expect("резерв обеспечен оркестратором");

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
                    self.orders.remove(id);
                }
                Event::OrderCanceledRemainder { id, .. } => {
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

/// Пересекается ли входящая заявка (сторона `side`, лимит `limit`) с ценой стоящей встречной `resting`.
fn crosses(side: Side, limit: Option<Price>, resting: Price) -> bool {
    match limit {
        None => true, // рыночная берёт любую цену
        Some(l) => match side {
            Side::Buy => resting <= l,   // своя Sell по цене ≤ лимита покупки
            Side::Sell => resting >= l,  // своя Buy по цене ≥ лимита продажи
        },
    }
}

/// Причина отказа движка для конкретной заявки, если она есть в событиях.
fn rejected_reason(events: &[Event], id: OrderId) -> Option<RejectReason> {
    events.iter().find_map(|e| match e {
        Event::OrderRejected { id: rid, reason, .. } if *rid == id => Some(*reason),
        _ => None,
    })
}
