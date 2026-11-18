use std::collections::{HashMap, HashSet};

use domain::account::UserId;
pub use domain::account::{
    AccountKind, AccountOrigin, AccountProfile, CategoryAtOpen, CategoryNow, ClientCategory,
    Currency,
};

pub mod journal;

use journal::{CloseCause, EventKind, RejectReason};

pub type Minor = i128;

pub type Cents = Minor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosSide {
    Long,
    Short,
}

fn value_minor(price_raw: i64, qty_raw: i64, pd: u8, qd: u8, minor_per_unit: i128) -> Minor {
    price_raw as i128 * qty_raw as i128 * minor_per_unit / 10i128.pow(pd as u32 + qd as u32)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PositionModel {
    #[default]
    Hedging,
    Netting,
}

pub fn merged_entry(qty_a: i64, entry_a: i64, qty_b: i64, entry_b: i64) -> i64 {
    let total = qty_a as i128 + qty_b as i128;
    if total == 0 {
        return entry_a;
    }
    let sum = entry_a as i128 * qty_a as i128 + entry_b as i128 * qty_b as i128;
    let half = total / 2;
    let rounded = if sum >= 0 {
        (sum + half) / total
    } else {
        (sum - half) / total
    };
    rounded as i64
}

pub fn realized_on_close(
    side: PosSide,
    entry: i64,
    exit: i64,
    qty: i64,
    pd: u8,
    qd: u8,
    minor_per_unit: i128,
) -> Minor {
    let diff = match side {
        PosSide::Long => exit - entry,
        PosSide::Short => entry - exit,
    };
    value_minor(diff, qty, pd, qd, minor_per_unit)
}

pub fn margin_for(entry: i64, qty: i64, pd: u8, qd: u8, lev: i128, minor_per_unit: i128) -> Minor {
    value_minor(entry, qty, pd, qd, minor_per_unit) / lev.max(1)
}

pub const MAX_FEE_BPS: i64 = 500;

const BPS: i128 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Fees {
    pub markup_bps: i64,
    pub commission_bps: i64,
}

impl Fees {
    pub fn is_zero(&self) -> bool {
        self.markup_bps == 0 && self.commission_bps == 0
    }

    pub fn validate(&self) -> Result<(), BrokerError> {
        let ok = |v: i64| (0..=MAX_FEE_BPS).contains(&v);
        if ok(self.markup_bps) && ok(self.commission_bps) {
            Ok(())
        } else {
            Err(BrokerError::InvalidFees)
        }
    }
}

pub fn apply_markup(buy: bool, market: i64, markup_bps: i64) -> i64 {
    if markup_bps <= 0 {
        return market;
    }
    let off = (market as i128 * markup_bps as i128 + BPS / 2) / BPS;
    let off = off as i64;
    if buy {
        market + off
    } else {
        market - off
    }
}

pub fn commission_for(
    exec: i64,
    qty: i64,
    pd: u8,
    qd: u8,
    commission_bps: i64,
    minor_per_unit: i128,
) -> Minor {
    if commission_bps <= 0 {
        return 0;
    }
    let notional = value_minor(exec, qty, pd, qd, minor_per_unit);
    (notional * commission_bps as i128 + BPS / 2) / BPS
}

pub fn markup_amount(market: i64, exec: i64, qty: i64, pd: u8, qd: u8, minor: i128) -> Minor {
    value_minor((exec - market).abs(), qty, pd, qd, minor)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PositionFees {
    pub markup_bps: i64,
    pub commission_bps: i64,
    pub commission: Minor,
    pub markup: Minor,
}

impl PositionFees {
    fn snapshot(f: Fees) -> Self {
        Self {
            markup_bps: f.markup_bps,
            commission_bps: f.commission_bps,
            commission: 0,
            markup: 0,
        }
    }
    fn rates(&self) -> Fees {
        Fees {
            markup_bps: self.markup_bps,
            commission_bps: self.commission_bps,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DealFees {
    pub commission: Minor,
    pub markup: Minor,
    pub market_exit: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    pub id: u64,
    pub instrument: u32,
    pub side: PosSide,
    pub qty: i64,
    pub entry: i64,
    pub pd: u8,
    pub qd: u8,
    pub margin: Cents,
    pub sl: Option<i64>,
    pub tp: Option<i64>,
    pub leverage: i128,
    pub fees: PositionFees,
}

impl Position {
    pub fn unrealized(&self, mark_raw: i64, minor_per_unit: i128) -> Minor {
        let diff = match self.side {
            PosSide::Long => mark_raw - self.entry,
            PosSide::Short => self.entry - mark_raw,
        };
        value_minor(diff, self.qty, self.pd, self.qd, minor_per_unit)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOrder {
    pub id: u64,
    pub instrument: u32,
    pub side: PosSide,
    pub qty: i64,
    pub price: i64,
    pub pd: u8,
    pub qd: u8,
    pub sl: Option<i64>,
    pub tp: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedDeal {
    pub instrument: u32,
    pub side: PosSide,
    pub qty: i64,
    pub entry: i64,
    pub exit: i64,
    pub pnl: Cents,
    pub pd: u8,
    pub qd: u8,
    pub level: Option<i64>,
    pub across_gap: Option<bool>,
    pub fees: DealFees,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerError {
    InsufficientMargin,
    UnknownPosition,
    UnknownPending,
    InsufficientFunds,
    UnknownAccount,
    NotADemoAccount,
    AccountNotFlat,
    StopLossAlreadyReached,
    TakeProfitAlreadyReached,
    InvalidLeverage,
    InvalidFees,
}

#[derive(Debug, Clone, Copy)]
struct Terms {
    lev: i128,
    fees: Fees,
    protect: bool,
}

pub const MAX_LEVERAGE: i128 = 1000;

pub fn stop_loss_reached(side: PosSide, m: i64, sl: i64) -> bool {
    match side {
        PosSide::Long => m <= sl,
        PosSide::Short => m >= sl,
    }
}

pub fn take_profit_reached(side: PosSide, m: i64, tp: i64) -> bool {
    match side {
        PosSide::Long => m >= tp,
        PosSide::Short => m <= tp,
    }
}

fn refuse_levels_already_reached(
    side: PosSide,
    at: i64,
    sl: Option<i64>,
    tp: Option<i64>,
) -> Result<(), BrokerError> {
    if sl.is_some_and(|sl| stop_loss_reached(side, at, sl)) {
        return Err(BrokerError::StopLossAlreadyReached);
    }
    if tp.is_some_and(|tp| take_profit_reached(side, at, tp)) {
        return Err(BrokerError::TakeProfitAlreadyReached);
    }
    Ok(())
}

#[derive(Debug, Default, Clone)]
struct Account {
    profile: AccountProfile,
    balance: Cents,
    next_id: u64,
    positions: HashMap<u64, Position>,
    pendings: HashMap<u64, PendingOrder>,
    closed: Vec<ClosedDeal>,
    model: PositionModel,
    margin_called: bool,
    leverage: Option<i128>,
    fees: Fees,
    outbox: Vec<EventKind>,
}

impl Account {
    fn minor(&self) -> i128 {
        self.profile.currency.minor_per_unit()
    }

    fn used_margin(&self) -> Cents {
        self.positions.values().map(|p| p.margin).sum()
    }

    fn open_pnl(&self, marks: &HashMap<u32, i64>) -> Cents {
        self.positions
            .values()
            .map(|p| p.unrealized(*marks.get(&p.instrument).unwrap_or(&p.entry), self.minor()))
            .sum()
    }

    fn equity(&self, marks: &HashMap<u32, i64>) -> Cents {
        self.balance + self.open_pnl(marks)
    }

    fn free_margin(&self, marks: &HashMap<u32, i64>) -> Cents {
        self.equity(marks) - self.used_margin()
    }

    fn note_margin_call(
        &mut self,
        marks: &HashMap<u32, i64>,
        call_pct: i128,
        rearm_band_pct: i128,
    ) -> Option<i128> {
        let Some(level) = self.margin_level_pct(marks) else {
            self.margin_called = false;
            return None;
        };
        if level > call_pct + rearm_band_pct {
            self.margin_called = false;
            return None;
        }
        if level > call_pct {
            return None;
        }
        if self.margin_called {
            return None;
        }
        self.margin_called = true;
        Some(level)
    }

    fn margin_level_pct(&self, marks: &HashMap<u32, i64>) -> Option<i128> {
        let used = self.used_margin();
        (used > 0).then(|| self.equity(marks) * 100 / used)
    }

    fn log(&mut self, e: EventKind) {
        self.outbox.push(e);
    }

    #[allow(clippy::too_many_arguments)]
    fn book_execution(
        &mut self,
        id: u64,
        market: i64,
        exec: i64,
        qty: i64,
        pd: u8,
        qd: u8,
        rates: Fees,
        protect: bool,
        realized: Minor,
    ) {
        let minor = self.minor();
        if rates.markup_bps > 0 {
            let amount = markup_amount(market, exec, qty, pd, qd, minor);
            if let Some(p) = self.positions.get_mut(&id) {
                p.fees.markup += amount;
            }
            self.log(EventKind::MarkupApplied {
                trade: id,
                mkt: market,
                exec,
                amount,
            });
        }
        let due = commission_for(exec, qty, pd, qd, rates.commission_bps, minor);
        if due > 0 {
            let amount = if protect {
                due.min((self.balance + realized).max(0))
            } else {
                due
            };
            self.balance -= amount;
            if let Some(p) = self.positions.get_mut(&id) {
                p.fees.commission += amount;
            }
            self.log(EventKind::CommissionCharged {
                amount,
                trade: id,
                waived: due - amount,
            });
        }
    }

    fn take_deal_fees(&mut self, id: u64, market: i64) -> DealFees {
        let p = self.positions.get_mut(&id).expect("позиция на месте");
        let f = DealFees {
            commission: p.fees.commission,
            markup: p.fees.markup,
            market_exit: Some(market),
        };
        p.fees.commission = 0;
        p.fees.markup = 0;
        f
    }

    #[allow(clippy::too_many_arguments)]
    fn try_open(
        &mut self,
        t: Terms,
        marks: &HashMap<u32, i64>,
        instrument: u32,
        side: PosSide,
        qty: i64,
        market: i64,
        pd: u8,
        qd: u8,
        sl: Option<i64>,
        tp: Option<i64>,
    ) -> Result<u64, BrokerError> {
        if let Some(&m) = marks.get(&instrument) {
            refuse_levels_already_reached(side, m, sl, tp)?;
        }
        match self.model {
            PositionModel::Hedging => {
                self.open_new(t, marks, instrument, side, qty, market, pd, qd, sl, tp)
            }
            PositionModel::Netting => {
                self.open_netting(t, marks, instrument, side, qty, market, pd, qd, sl, tp)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn open_new(
        &mut self,
        t: Terms,
        marks: &HashMap<u32, i64>,
        instrument: u32,
        side: PosSide,
        qty: i64,
        market: i64,
        pd: u8,
        qd: u8,
        sl: Option<i64>,
        tp: Option<i64>,
    ) -> Result<u64, BrokerError> {
        let lev = t.lev;
        let exec = apply_markup(side == PosSide::Long, market, t.fees.markup_bps);
        let margin = margin_for(exec, qty, pd, qd, lev, self.minor());
        let commission = commission_for(exec, qty, pd, qd, t.fees.commission_bps, self.minor());
        if self.free_margin(marks) < margin + commission {
            return Err(BrokerError::InsufficientMargin);
        }
        self.next_id += 1;
        let id = self.next_id;
        self.positions.insert(
            id,
            Position {
                id,
                instrument,
                side,
                qty,
                entry: exec,
                pd,
                qd,
                margin,
                sl,
                tp,
                leverage: lev,
                fees: PositionFees::snapshot(t.fees),
            },
        );
        self.log(EventKind::PositionOpened {
            id,
            instrument,
            side,
            qty,
            entry: exec,
            pd,
            qd,
            sl,
            tp,
            lev,
        });
        self.book_execution(id, market, exec, qty, pd, qd, t.fees, t.protect, 0);
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    fn open_netting(
        &mut self,
        t: Terms,
        marks: &HashMap<u32, i64>,
        instrument: u32,
        side: PosSide,
        qty: i64,
        market: i64,
        pd: u8,
        qd: u8,
        sl: Option<i64>,
        tp: Option<i64>,
    ) -> Result<u64, BrokerError> {
        let existing = self
            .positions
            .values()
            .find(|p| p.instrument == instrument)
            .map(|p| p.id);
        let Some(pos_id) = existing else {
            return self.open_new(t, marks, instrument, side, qty, market, pd, qd, sl, tp);
        };
        let cur = self
            .positions
            .get(&pos_id)
            .expect("позиция только что найдена")
            .clone();
        let rates = cur.fees.rates();
        let exec = apply_markup(side == PosSide::Long, market, rates.markup_bps);

        if cur.side == side {
            let new_qty = cur.qty + qty;
            let new_entry = merged_entry(cur.qty, cur.entry, qty, exec);
            let new_margin = margin_for(new_entry, new_qty, pd, qd, cur.leverage, self.minor());
            let extra = new_margin - cur.margin;
            let commission = commission_for(exec, qty, pd, qd, rates.commission_bps, self.minor());
            let need = extra.max(0) + commission;
            if need > 0 && self.free_margin(marks) < need {
                return Err(BrokerError::InsufficientMargin);
            }
            let p = self.positions.get_mut(&pos_id).expect("позиция на месте");
            p.qty = new_qty;
            p.entry = new_entry;
            p.margin = new_margin;
            if sl.is_some() {
                p.sl = sl;
            }
            if tp.is_some() {
                p.tp = tp;
            }
            self.log(EventKind::PositionIncreased {
                id: pos_id,
                add_qty: qty,
                add_entry: exec,
                sl,
                tp,
            });
            self.book_execution(pos_id, market, exec, qty, pd, qd, rates, t.protect, 0);
            return Ok(pos_id);
        }

        let closing = qty.min(cur.qty);
        let pnl = realized_on_close(
            cur.side,
            cur.entry,
            exec,
            closing,
            cur.pd,
            cur.qd,
            self.minor(),
        );
        self.book_execution(
            pos_id, market, exec, closing, cur.pd, cur.qd, rates, t.protect, pnl,
        );
        let fees = self.take_deal_fees(pos_id, market);
        self.balance += pnl;
        self.closed.push(ClosedDeal {
            instrument: cur.instrument,
            side: cur.side,
            qty: closing,
            entry: cur.entry,
            exit: exec,
            pnl,
            pd: cur.pd,
            qd: cur.qd,
            level: None,
            across_gap: Some(false),
            fees,
        });

        let left = cur.qty - closing;
        if left > 0 {
            let minor = self.minor();
            let p = self.positions.get_mut(&pos_id).expect("позиция на месте");
            p.qty = left;
            p.margin = margin_for(p.entry, left, p.pd, p.qd, p.leverage, minor);
            self.log(EventKind::PositionReduced {
                id: pos_id,
                qty: closing,
                exit: exec,
            });
            return Ok(pos_id);
        }

        self.positions.remove(&pos_id);
        self.log(EventKind::PositionClosed {
            id: pos_id,
            exit: exec,
            cause: CloseCause::Netting,
            level: None,
            across_gap: false,
        });
        let remainder = qty - closing;
        if remainder > 0 {
            return self.open_new(
                t, marks, instrument, side, remainder, market, pd, qd, sl, tp,
            );
        }
        Ok(pos_id)
    }

    fn close_pos(
        &mut self,
        id: u64,
        market: i64,
        level: Option<i64>,
        across_gap: bool,
        cause: CloseCause,
        protect: bool,
    ) -> Option<Cents> {
        let minor = self.minor();
        let pos = self.positions.get(&id)?.clone();
        let rates = pos.fees.rates();
        let exec = apply_markup(pos.side == PosSide::Short, market, rates.markup_bps);
        let pnl = pos.unrealized(exec, minor);
        self.book_execution(
            id, market, exec, pos.qty, pos.pd, pos.qd, rates, protect, pnl,
        );
        let fees = self.take_deal_fees(id, market);
        self.positions.remove(&id);
        self.balance += pnl;
        self.closed.push(ClosedDeal {
            instrument: pos.instrument,
            side: pos.side,
            qty: pos.qty,
            entry: pos.entry,
            exit: exec,
            pnl,
            pd: pos.pd,
            qd: pos.qd,
            level,
            across_gap: Some(across_gap),
            fees,
        });
        self.log(EventKind::PositionClosed {
            id,
            exit: exec,
            cause,
            level,
            across_gap,
        });
        Some(pnl)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_triggers(
        &mut self,
        t: Terms,
        marks: &HashMap<u32, i64>,
        across_gap: bool,
        stop_out_pct: Option<i128>,
        frozen: bool,
    ) -> bool {
        let protect_negative = t.protect;
        let mut fire: Vec<PendingOrder> = Vec::new();
        self.pendings.retain(|_, p| match marks.get(&p.instrument) {
            Some(&m) => {
                let hit = match p.side {
                    PosSide::Long => m <= p.price,
                    PosSide::Short => m >= p.price,
                };
                if hit {
                    fire.push(p.clone());
                    false
                } else {
                    true
                }
            }
            None => true,
        });
        let fired_any = !fire.is_empty();
        for p in fire {
            let exec = marks.get(&p.instrument).copied().unwrap_or(p.price);
            if frozen {
                self.log(EventKind::PendingRejected {
                    id: p.id,
                    exec: Some(exec),
                    reason: RejectReason::AccountFrozen,
                });
                continue;
            }
            self.log(EventKind::PendingTriggered {
                id: p.id,
                price: p.price,
                exec: Some(exec),
                across_gap,
            });
            let _ = self.try_open(
                t,
                marks,
                p.instrument,
                p.side,
                p.qty,
                exec,
                p.pd,
                p.qd,
                p.sl,
                p.tp,
            );
        }

        let mut to_close: Vec<(u64, i64, i64, CloseCause)> = Vec::new();
        for pos in self.positions.values() {
            if let Some(&m) = marks.get(&pos.instrument) {
                let sl_hit = pos.sl.is_some_and(|sl| stop_loss_reached(pos.side, m, sl));
                let tp_hit = pos
                    .tp
                    .is_some_and(|tp| take_profit_reached(pos.side, m, tp));
                if sl_hit {
                    to_close.push((pos.id, m, pos.sl.unwrap(), CloseCause::StopLoss));
                } else if tp_hit {
                    to_close.push((pos.id, m, pos.tp.unwrap(), CloseCause::TakeProfit));
                }
            }
        }
        let closed_any = !to_close.is_empty();
        for (id, exec, level, cause) in to_close {
            self.close_pos(id, exec, Some(level), across_gap, cause, t.protect);
        }

        let stopped_out = self.run_stop_out(marks, across_gap, stop_out_pct, t.protect);
        let compensated = self.cover_negative_balance(protect_negative);

        fired_any || closed_any || stopped_out || compensated
    }

    fn cover_negative_balance(&mut self, enabled: bool) -> bool {
        if !enabled || self.balance >= 0 || !self.positions.is_empty() {
            return false;
        }
        let amount = -self.balance;
        self.balance += amount;
        self.log(EventKind::NegativeBalanceCompensated { amount });
        true
    }

    fn run_stop_out(
        &mut self,
        marks: &HashMap<u32, i64>,
        across_gap: bool,
        stop_out_pct: Option<i128>,
        protect: bool,
    ) -> bool {
        let Some(threshold) = stop_out_pct else {
            return false;
        };
        let minor = self.minor();
        let mut closed = false;
        loop {
            let used = self.used_margin();
            if used <= 0 {
                break;
            }
            if self.equity(marks) * 100 > threshold * used {
                break;
            }
            let worst = self
                .positions
                .values()
                .filter_map(|p| {
                    marks
                        .get(&p.instrument)
                        .map(|m| (p.id, *m, p.unrealized(*m, minor)))
                })
                .min_by_key(|(_, _, pnl)| *pnl);
            let Some((id, mark, _)) = worst else {
                break;
            };
            self.close_pos(id, mark, None, across_gap, CloseCause::StopOut, protect);
            closed = true;
        }
        closed
    }
}

pub struct Broker {
    start_balance: Cents,
    leverage: i128,
    accounts: HashMap<UserId, Account>,
    stop_out_pct: Option<i128>,
    protect_negative_balance: bool,
    margin_call_pct: Option<i128>,
    margin_call_rearm_band_pct: i128,
}

impl Broker {
    pub fn new(start_balance: Cents, leverage: i128) -> Self {
        Self {
            start_balance,
            leverage: leverage.max(1),
            accounts: HashMap::new(),
            stop_out_pct: None,
            margin_call_pct: None,
            margin_call_rearm_band_pct: 0,
            protect_negative_balance: false,
        }
    }

    pub fn with_stop_out_pct(mut self, pct: Option<i128>) -> Self {
        self.stop_out_pct = pct;
        self
    }

    pub fn stop_out_pct(&self) -> Option<i128> {
        self.stop_out_pct
    }

    pub fn with_margin_call_pct(mut self, pct: Option<i128>) -> Self {
        self.margin_call_pct = pct;
        self
    }

    pub fn margin_call_pct(&self) -> Option<i128> {
        self.margin_call_pct
    }

    pub fn with_margin_call_rearm_band_pct(mut self, pct: i128) -> Self {
        self.margin_call_rearm_band_pct = pct.max(0);
        self
    }

    pub fn margin_call_rearm_band_pct(&self) -> i128 {
        self.margin_call_rearm_band_pct
    }

    pub fn with_negative_balance_protection(mut self, on: bool) -> Self {
        self.protect_negative_balance = on;
        self
    }

    pub fn negative_balance_protection(&self) -> bool {
        self.protect_negative_balance
    }

    pub fn margin_calls(&mut self, marks: &HashMap<u32, i64>) -> Vec<(UserId, i128)> {
        let Some(call) = self.margin_call_pct else {
            return Vec::new();
        };
        let band = self.margin_call_rearm_band_pct;
        let mut out = Vec::new();
        for (user, a) in self.accounts.iter_mut() {
            if let Some(level) = a.note_margin_call(marks, call, band) {
                out.push((*user, level));
            }
        }
        out.sort_by_key(|(u, _)| u.0);
        out
    }

    fn acct(&mut self, user: UserId) -> &mut Account {
        let start = self.start_balance;
        let profile = AccountProfile::legacy_demo(user);
        self.accounts.entry(user).or_insert_with(|| {
            let mut a = Account {
                balance: start,
                profile,
                ..Default::default()
            };
            a.log(EventKind::AccountOpened {
                balance: start,
                profile,
            });
            a
        })
    }

    pub fn has_account(&self, user: UserId) -> bool {
        self.accounts.contains_key(&user)
    }

    pub fn open_account(&mut self, user: UserId) -> bool {
        self.open_account_with(user, AccountProfile::legacy_demo(user), self.start_balance)
    }

    pub fn open_account_with(
        &mut self,
        user: UserId,
        profile: AccountProfile,
        demo_start: Cents,
    ) -> bool {
        if self.accounts.contains_key(&user) {
            return false;
        }
        let start = start_balance_for(profile.kind, demo_start);
        let mut a = Account {
            balance: start,
            profile,
            ..Default::default()
        };
        a.log(EventKind::AccountOpened {
            balance: start,
            profile,
        });
        self.accounts.insert(user, a);
        true
    }

    pub fn forget(&mut self, user: UserId) -> bool {
        self.accounts.remove(&user).is_some()
    }

    pub fn take_events(&mut self, user: UserId) -> Vec<EventKind> {
        self.accounts
            .get_mut(&user)
            .map(|a| std::mem::take(&mut a.outbox))
            .unwrap_or_default()
    }

    pub fn pending_events(&self, user: UserId) -> usize {
        self.accounts
            .get(&user)
            .map(|a| a.outbox.len())
            .unwrap_or(0)
    }

    pub fn balance(&self, user: UserId) -> Option<Cents> {
        self.accounts.get(&user).map(|a| a.balance)
    }
    pub fn used_margin(&self, user: UserId) -> Cents {
        self.accounts
            .get(&user)
            .map(|a| a.used_margin())
            .unwrap_or(0)
    }
    pub fn free_margin(&self, user: UserId, marks: &HashMap<u32, i64>) -> Option<Cents> {
        self.accounts.get(&user).map(|a| a.free_margin(marks))
    }

    pub fn margin_level_pct(&self, user: UserId, marks: &HashMap<u32, i64>) -> Option<i128> {
        self.accounts
            .get(&user)
            .and_then(|a| a.margin_level_pct(marks))
    }
    pub fn positions(&self, user: UserId) -> Vec<Position> {
        self.accounts
            .get(&user)
            .map(|a| a.positions.values().cloned().collect())
            .unwrap_or_default()
    }
    pub fn pendings(&self, user: UserId) -> Vec<PendingOrder> {
        self.accounts
            .get(&user)
            .map(|a| a.pendings.values().cloned().collect())
            .unwrap_or_default()
    }
    pub fn closed_deals(&self, user: UserId) -> Vec<ClosedDeal> {
        self.accounts
            .get(&user)
            .map(|a| a.closed.clone())
            .unwrap_or_default()
    }
    pub fn open_pnl(&self, user: UserId, marks: &HashMap<u32, i64>) -> Cents {
        self.accounts
            .get(&user)
            .map(|a| {
                a.positions
                    .values()
                    .map(|p| {
                        p.unrealized(
                            *marks.get(&p.instrument).unwrap_or(&p.entry),
                            a.profile.currency.minor_per_unit(),
                        )
                    })
                    .sum()
            })
            .unwrap_or(0)
    }
    pub fn equity(&self, user: UserId, marks: &HashMap<u32, i64>) -> Option<Cents> {
        self.balance(user).map(|b| b + self.open_pnl(user, marks))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open(
        &mut self,
        user: UserId,
        marks: &HashMap<u32, i64>,
        instrument: u32,
        side: PosSide,
        qty: i64,
        entry: i64,
        pd: u8,
        qd: u8,
        sl: Option<i64>,
        tp: Option<i64>,
    ) -> Result<u64, BrokerError> {
        let platform = self.leverage;
        let protect = self.protect_negative_balance;
        let a = self.acct(user);
        let t = Terms {
            lev: a.leverage.unwrap_or(platform),
            fees: a.fees,
            protect,
        };
        a.try_open(t, marks, instrument, side, qty, entry, pd, qd, sl, tp)
    }

    pub fn close(&mut self, user: UserId, id: u64, mark: i64) -> Result<Cents, BrokerError> {
        let protect = self.protect_negative_balance;
        self.acct(user)
            .close_pos(id, mark, None, false, CloseCause::Command, protect)
            .ok_or(BrokerError::UnknownPosition)
    }

    pub fn deposit(&mut self, user: UserId, amount: Cents) -> Cents {
        let a = self.acct(user);
        a.balance += amount;
        a.log(EventKind::Deposited { amount });
        a.balance
    }

    pub fn withdraw(
        &mut self,
        user: UserId,
        amount: Cents,
        marks: &HashMap<u32, i64>,
    ) -> Result<Cents, BrokerError> {
        let Some(free) = self.free_margin(user, marks) else {
            return Err(BrokerError::InsufficientFunds);
        };
        if amount > free {
            return Err(BrokerError::InsufficientFunds);
        }
        let a = self.acct(user);
        a.balance -= amount;
        a.log(EventKind::Withdrawn { amount });
        Ok(a.balance)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn place_pending(
        &mut self,
        user: UserId,
        instrument: u32,
        side: PosSide,
        qty: i64,
        price: i64,
        pd: u8,
        qd: u8,
        sl: Option<i64>,
        tp: Option<i64>,
    ) -> Result<u64, BrokerError> {
        refuse_levels_already_reached(side, price, sl, tp)?;
        let a = self.acct(user);
        a.next_id += 1;
        let id = a.next_id;
        a.pendings.insert(
            id,
            PendingOrder {
                id,
                instrument,
                side,
                qty,
                price,
                pd,
                qd,
                sl,
                tp,
            },
        );
        a.log(EventKind::PendingPlaced {
            id,
            instrument,
            side,
            qty,
            price,
            pd,
            qd,
            sl,
            tp,
        });
        Ok(id)
    }

    pub fn cancel_pending(&mut self, user: UserId, id: u64) -> Result<(), BrokerError> {
        let a = self.acct(user);
        if a.pendings.remove(&id).is_some() {
            a.log(EventKind::PendingCancelled { id });
            Ok(())
        } else {
            Err(BrokerError::UnknownPending)
        }
    }

    pub fn set_position_levels(
        &mut self,
        user: UserId,
        id: u64,
        sl: Option<i64>,
        tp: Option<i64>,
        at: &[i64],
    ) -> Result<bool, BrokerError> {
        let a = self
            .accounts
            .get_mut(&user)
            .ok_or(BrokerError::UnknownPosition)?;
        let p = a
            .positions
            .get_mut(&id)
            .ok_or(BrokerError::UnknownPosition)?;
        if p.sl == sl && p.tp == tp {
            return Ok(false);
        }
        let new_sl = if p.sl != sl { sl } else { None };
        let new_tp = if p.tp != tp { tp } else { None };
        for &m in at {
            refuse_levels_already_reached(p.side, m, new_sl, new_tp)?;
        }
        p.sl = sl;
        p.tp = tp;
        a.log(EventKind::PositionLevelsChanged { id, sl, tp });
        Ok(true)
    }

    pub fn set_pending_levels(
        &mut self,
        user: UserId,
        id: u64,
        sl: Option<i64>,
        tp: Option<i64>,
    ) -> Result<bool, BrokerError> {
        let a = self
            .accounts
            .get_mut(&user)
            .ok_or(BrokerError::UnknownPending)?;
        let o = a.pendings.get_mut(&id).ok_or(BrokerError::UnknownPending)?;
        if o.sl == sl && o.tp == tp {
            return Ok(false);
        }
        let new_sl = if o.sl != sl { sl } else { None };
        let new_tp = if o.tp != tp { tp } else { None };
        refuse_levels_already_reached(o.side, o.price, new_sl, new_tp)?;
        o.sl = sl;
        o.tp = tp;
        a.log(EventKind::PendingLevelsChanged { id, sl, tp });
        Ok(true)
    }

    pub fn check(&mut self, marks: &HashMap<u32, i64>, across_gap: bool) -> Vec<UserId> {
        self.check_with_frozen(marks, across_gap, &HashSet::new())
    }

    pub fn check_with_frozen(
        &mut self,
        marks: &HashMap<u32, i64>,
        across_gap: bool,
        frozen: &HashSet<UserId>,
    ) -> Vec<UserId> {
        let platform = self.leverage;
        let stop_out = self.stop_out_pct;
        let protect = self.protect_negative_balance;
        let mut changed = Vec::new();
        for (user, a) in self.accounts.iter_mut() {
            let t = Terms {
                lev: a.leverage.unwrap_or(platform),
                fees: a.fees,
                protect,
            };
            let is_frozen = frozen.contains(user);
            if a.run_triggers(t, marks, across_gap, stop_out, is_frozen) {
                changed.push(*user);
            }
        }
        changed
    }

    pub fn snapshot(&self, user: UserId) -> AccountSnapshot {
        match self.accounts.get(&user) {
            Some(a) => AccountSnapshot {
                profile: a.profile,
                balance: a.balance,
                next_id: a.next_id,
                positions: a.positions.values().cloned().collect(),
                pendings: a.pendings.values().cloned().collect(),
                closed: a.closed.clone(),
                model: a.model,
                leverage: a.leverage,
                fees: a.fees,
            },
            None => AccountSnapshot {
                profile: AccountProfile::legacy_demo(user),
                balance: self.start_balance,
                next_id: 0,
                positions: vec![],
                pendings: vec![],
                closed: vec![],
                model: PositionModel::default(),
                leverage: None,
                fees: Fees::default(),
            },
        }
    }

    pub fn restore(&mut self, user: UserId, snap: AccountSnapshot) {
        let a = Account {
            profile: snap.profile,
            balance: snap.balance,
            next_id: snap.next_id,
            positions: snap.positions.into_iter().map(|p| (p.id, p)).collect(),
            pendings: snap.pendings.into_iter().map(|p| (p.id, p)).collect(),
            closed: snap.closed,
            model: snap.model,
            leverage: snap.leverage,
            fees: snap.fees,
            margin_called: false,
            outbox: Vec::new(),
        };
        self.accounts.insert(user, a);
    }

    pub fn reset_demo(&mut self, user: UserId) -> Result<Cents, BrokerError> {
        let start = self.start_balance;
        let a = self
            .accounts
            .get_mut(&user)
            .ok_or(BrokerError::UnknownAccount)?;
        if a.profile.kind != AccountKind::Demo {
            return Err(BrokerError::NotADemoAccount);
        }
        if !a.positions.is_empty() || !a.pendings.is_empty() {
            return Err(BrokerError::AccountNotFlat);
        }
        let target = start_balance_for(a.profile.kind, start);
        let adjustment = target - a.balance;
        if adjustment == 0 {
            return Ok(a.balance);
        }
        a.balance += adjustment;
        a.log(EventKind::DemoAccountReset { adjustment });
        Ok(a.balance)
    }

    pub fn adopt_model(&mut self, user: UserId, model: PositionModel) -> bool {
        let a = self.acct(user);
        if a.model == model {
            return true;
        }
        if a.positions.is_empty() {
            a.model = model;
            a.log(EventKind::ModelAdopted { model });
            true
        } else {
            false
        }
    }

    pub fn model(&self, user: UserId) -> PositionModel {
        self.accounts
            .get(&user)
            .map(|a| a.model)
            .unwrap_or_default()
    }

    pub fn leverage(&self) -> i128 {
        self.leverage
    }

    pub fn account_leverage(&self, user: UserId) -> i128 {
        self.assigned_leverage(user).unwrap_or(self.leverage)
    }

    pub fn assigned_leverage(&self, user: UserId) -> Option<i128> {
        self.accounts.get(&user).and_then(|a| a.leverage)
    }

    pub fn set_account_leverage(&mut self, user: UserId, lev: i128) -> Result<bool, BrokerError> {
        if !(1..=MAX_LEVERAGE).contains(&lev) {
            return Err(BrokerError::InvalidLeverage);
        }
        let a = self
            .accounts
            .get_mut(&user)
            .ok_or(BrokerError::UnknownAccount)?;
        if a.leverage == Some(lev) {
            return Ok(false);
        }
        a.leverage = Some(lev);
        a.log(EventKind::LeverageSet { lev });
        Ok(true)
    }

    pub fn account_fees(&self, user: UserId) -> Fees {
        self.accounts.get(&user).map(|a| a.fees).unwrap_or_default()
    }

    pub fn set_account_fees(&mut self, user: UserId, fees: Fees) -> Result<bool, BrokerError> {
        fees.validate()?;
        let a = self
            .accounts
            .get_mut(&user)
            .ok_or(BrokerError::UnknownAccount)?;
        if a.fees == fees {
            return Ok(false);
        }
        a.fees = fees;
        a.log(EventKind::FeesSet {
            markup_bps: fees.markup_bps,
            commission_bps: fees.commission_bps,
        });
        Ok(true)
    }

    pub fn set_leverage(&mut self, lev: i128) -> bool {
        if lev <= 0 {
            return false;
        }
        self.leverage = lev;
        true
    }

    pub fn start_balance(&self) -> Cents {
        self.start_balance
    }

    pub fn users(&self) -> Vec<UserId> {
        self.accounts.keys().copied().collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct AccountSnapshot {
    pub profile: AccountProfile,
    pub balance: Cents,
    pub next_id: u64,
    pub positions: Vec<Position>,
    pub pendings: Vec<PendingOrder>,
    pub closed: Vec<ClosedDeal>,
    pub model: PositionModel,
    pub leverage: Option<i128>,
    pub fees: Fees,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation(pub String);

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn start_balance_for(kind: AccountKind, demo_start: Cents) -> Cents {
    match kind {
        AccountKind::Demo => demo_start,
        AccountKind::Real => 0,
    }
}

pub fn check_snapshot(snap: &AccountSnapshot) -> Vec<Violation> {
    let mut out = Vec::new();
    let v = |out: &mut Vec<Violation>, msg: String| out.push(Violation(msg));

    let minor = snap.profile.currency.minor_per_unit();
    for p in &snap.positions {
        let lev = p.leverage.max(1);
        let expected = value_minor(p.entry, p.qty, p.pd, p.qd, minor) / lev;
        if p.margin != expected {
            v(
                &mut out,
                format!(
                    "позиция {}: маржа {} != пересчитанной {expected} (плечо {lev})",
                    p.id, p.margin
                ),
            );
        }
    }

    for p in &snap.positions {
        if p.qty <= 0 {
            v(
                &mut out,
                format!("позиция {}: объём {} не больше нуля", p.id, p.qty),
            );
        }
        if p.entry <= 0 {
            v(
                &mut out,
                format!("позиция {}: цена входа {} не больше нуля", p.id, p.entry),
            );
        }
        if p.margin < 0 {
            v(
                &mut out,
                format!("позиция {}: отрицательная маржа {}", p.id, p.margin),
            );
        }
    }

    if snap.fees.validate().is_err() {
        v(
            &mut out,
            format!("тариф счёта вне 0..={MAX_FEE_BPS} bps: {:?}", snap.fees),
        );
    }
    for p in &snap.positions {
        if p.fees.rates().validate().is_err() {
            v(
                &mut out,
                format!("позиция {}: тариф вне 0..={MAX_FEE_BPS} bps", p.id),
            );
        }
        if p.fees.commission < 0 || p.fees.markup < 0 {
            v(
                &mut out,
                format!("позиция {}: отрицательный накопленный доход площадки", p.id),
            );
        }
    }

    for o in &snap.pendings {
        if o.qty <= 0 {
            v(
                &mut out,
                format!("ордер {}: объём {} не больше нуля", o.id, o.qty),
            );
        }
        if o.price <= 0 {
            v(
                &mut out,
                format!("ордер {}: цена {} не больше нуля", o.id, o.price),
            );
        }
    }

    let mut ids: Vec<u64> = snap
        .positions
        .iter()
        .map(|p| p.id)
        .chain(snap.pendings.iter().map(|o| o.id))
        .collect();
    ids.sort_unstable();
    let total = ids.len();
    ids.dedup();
    if ids.len() != total {
        v(
            &mut out,
            "идентификаторы позиций и ордеров не уникальны".to_string(),
        );
    }
    if let Some(max) = ids.last() {
        if *max > snap.next_id {
            v(
                &mut out,
                format!("выдан id {max}, больший счётчика next_id {}", snap.next_id),
            );
        }
    }

    out
}

#[cfg(test)]
fn no_marks() -> HashMap<u32, i64> {
    HashMap::new()
}

#[cfg(test)]
mod invariant_tests {
    use super::*;

    fn pos(id: u64, qty: i64, entry: i64, margin: Cents) -> Position {
        Position {
            id,
            instrument: 1,
            side: PosSide::Long,
            qty,
            entry,
            pd: 2,
            qd: 3,
            margin,
            sl: None,
            tp: None,
            leverage: 1,
            fees: Default::default(),
        }
    }

    fn healthy() -> AccountSnapshot {
        AccountSnapshot {
            profile: Default::default(),
            balance: 1_000_000,
            next_id: 2,
            positions: vec![
                pos(1, 100, 5000, value_minor(5000, 100, 2, 3, 100)),
                pos(2, 200, 4000, value_minor(4000, 200, 2, 3, 100)),
            ],
            pendings: vec![],
            closed: vec![],
            model: PositionModel::Hedging,
            leverage: None,
            fees: Default::default(),
        }
    }

    #[test]
    fn healthy_snapshot_has_no_violations() {
        assert_eq!(check_snapshot(&healthy()), vec![]);
    }

    #[test]
    fn empty_account_is_valid() {
        assert_eq!(check_snapshot(&AccountSnapshot::default()), vec![]);
    }

    #[test]
    fn negative_balance_is_not_a_violation() {
        let mut s = healthy();
        s.balance = -50_000;
        assert_eq!(
            check_snapshot(&s),
            vec![],
            "отрицательный баланс законен с плечом"
        );
    }

    #[test]
    fn detects_zero_and_negative_qty() {
        let mut s = healthy();
        s.positions[0].qty = 0;
        assert!(
            check_snapshot(&s).iter().any(|v| v.0.contains("объём")),
            "нулевой объём должен ловиться"
        );
        s.positions[0].qty = -5;
        assert!(check_snapshot(&s).iter().any(|v| v.0.contains("объём")));
    }

    #[test]
    fn detects_zero_entry_price() {
        let mut s = healthy();
        s.positions[1].entry = 0;
        assert!(check_snapshot(&s)
            .iter()
            .any(|v| v.0.contains("цена входа")));
    }

    #[test]
    fn detects_margin_drift() {
        let mut s = healthy();
        s.positions[0].margin += 1;
        assert!(
            check_snapshot(&s).iter().any(|v| v.0.contains("маржа")),
            "дрейф маржи должен ловиться"
        );
    }

    #[test]
    fn detects_margin_inconsistent_with_position_leverage() {
        let mut s = healthy();
        s.positions[0].leverage = 10;
        assert!(
            !check_snapshot(&s).is_empty(),
            "маржа под плечо 1 не годится для позиции, записанной с плечом 10"
        );
    }

    #[test]
    fn platform_leverage_change_leaves_open_margin_alone() {
        let mut b = Broker::new(1_000_000, 1);
        let u = UserId(1);
        b.open_account(u);
        let m = HashMap::from([(1u32, 5000i64)]);
        let id = b
            .open(u, &m, 1, PosSide::Long, 1000, 5000, 2, 3, None, None)
            .expect("позиция открыта");
        let before = b.used_margin(u);
        assert!(before > 0, "предпосылка: маржа занята");

        assert!(b.set_leverage(100), "плечо площадки сменилось");

        assert_eq!(
            b.used_margin(u),
            before,
            "смена настройки площадки не двигает занятое под открытой позицией"
        );
        assert_eq!(
            b.snapshot(u)
                .positions
                .iter()
                .find(|p| p.id == id)
                .unwrap()
                .leverage,
            1,
            "позиция помнит плечо, под которым открыта"
        );
        assert_eq!(
            check_snapshot(&b.snapshot(u)),
            vec![],
            "слепок сходится сам с собой и после смены настройки"
        );
    }

    #[test]
    fn adding_to_a_position_after_a_leverage_change_keeps_its_leverage() {
        let mut b = Broker::new(1_000_000, 1);
        let u = UserId(1);
        b.open_account(u);
        assert!(
            b.adopt_model(u, PositionModel::Netting),
            "предпосылка: счёт неттинговый"
        );
        let m = HashMap::from([(1u32, 5000i64)]);
        b.open(u, &m, 1, PosSide::Long, 1000, 5000, 2, 3, None, None)
            .expect("позиция открыта");
        let one = b.used_margin(u);

        assert!(b.set_leverage(100), "плечо площадки сменилось");
        b.open(u, &m, 1, PosSide::Long, 1000, 5000, 2, 3, None, None)
            .expect("доливка прошла");

        assert_eq!(
            b.snapshot(u).positions.len(),
            1,
            "предпосылка: это доливка одной позиции, а не вторая позиция"
        );
        assert_eq!(
            b.used_margin(u),
            one * 2,
            "долитая половина обеспечена тем же плечом, что и первая"
        );
        assert_eq!(
            check_snapshot(&b.snapshot(u)),
            vec![],
            "слепок сходится: маржа отвечает плечу, записанному в позиции"
        );
    }

    #[test]
    fn positions_opened_around_a_leverage_change_keep_their_own() {
        let mut b = Broker::new(1_000_000, 1);
        let u = UserId(1);
        b.open_account(u);
        let m = HashMap::from([(1u32, 5000i64)]);
        b.open(u, &m, 1, PosSide::Long, 1000, 5000, 2, 3, None, None)
            .expect("первая позиция");
        assert!(b.set_leverage(100));
        b.open(u, &m, 1, PosSide::Long, 1000, 5000, 2, 3, None, None)
            .expect("вторая позиция");

        let snap = b.snapshot(u);
        let mut levs: Vec<i128> = snap.positions.iter().map(|p| p.leverage).collect();
        levs.sort_unstable();
        assert_eq!(levs, vec![1, 100], "каждая позиция помнит своё плечо");
        assert_eq!(
            b.used_margin(u),
            5050,
            "занято 5000 под первой (плечо 1) и 50 под второй (плечо 100)"
        );
        assert_eq!(
            check_snapshot(&snap),
            vec![],
            "счёт с двумя плечами — здоровый счёт, а не карантин"
        );
    }

    #[test]
    fn set_leverage_refuses_nonpositive() {
        let mut b = Broker::new(1_000_000, 7);
        for bad in [0i128, -5] {
            assert!(!b.set_leverage(bad), "плечо {bad} не принимается");
            assert_eq!(b.leverage(), 7, "после отказа плечо прежнее");
        }
        assert!(b.set_leverage(50), "нормальное значение принимается");
        assert_eq!(b.leverage(), 50);
    }

    #[test]
    fn margin_level_half_is_half_at_any_leverage() {
        for (lev, want_used) in [(1i128, 5000i128), (100, 50)] {
            let mut b = Broker::new(1_000_000, lev);
            let u = UserId(1);
            b.open_account(u);
            let m = HashMap::from([(1u32, 5000i64)]);
            b.open(u, &m, 1, PosSide::Long, 1000, 5000, 2, 3, None, None)
                .expect("позиция открыта");
            let used = b.used_margin(u);
            assert_eq!(
                used, want_used,
                "плечо {lev}: занято должно быть {want_used}"
            );
            let want_equity = used / 2;
            let bal = b.balance(u).expect("счёт заведён");
            let need_pnl = want_equity - bal;
            let unit = b.open_pnl(u, &HashMap::from([(1u32, 5001i64)]));
            assert!(unit != 0, "предпосылка: тик цены двигает P&L");
            let mark = 5000 + (need_pnl / unit) as i64;
            let marks = HashMap::from([(1u32, mark)]);
            let pct = b.margin_level_pct(u, &marks).expect("уровень маржи есть");
            assert!(
                (pct - 50).abs() <= 1,
                "плечо {lev}: уровень маржи {pct}%, ожидали ~50%"
            );
        }
    }

    #[test]
    fn detects_negative_margin() {
        let mut s = healthy();
        s.positions[0].margin = -1;
        assert!(check_snapshot(&s)
            .iter()
            .any(|v| v.0.contains("отрицательная маржа")));
    }

    #[test]
    fn detects_duplicate_ids() {
        let mut s = healthy();
        s.positions[1].id = s.positions[0].id;
        assert!(check_snapshot(&s)
            .iter()
            .any(|v| v.0.contains("не уникальны")));
    }

    #[test]
    fn detects_id_beyond_counter() {
        let mut s = healthy();
        s.next_id = 1;
        assert!(check_snapshot(&s).iter().any(|v| v.0.contains("next_id")));
    }

    #[test]
    fn detects_bad_pending_order() {
        let mut s = healthy();
        s.next_id = 3;
        s.pendings.push(PendingOrder {
            id: 3,
            instrument: 1,
            side: PosSide::Long,
            qty: 0,
            price: 100,
            pd: 2,
            qd: 3,
            sl: None,
            tp: None,
        });
        assert!(check_snapshot(&s).iter().any(|v| v.0.contains("ордер")));
        s.pendings[0].qty = 10;
        s.pendings[0].price = 0;
        assert!(check_snapshot(&s).iter().any(|v| v.0.contains("цена")));
    }

    #[test]
    fn invariants_do_not_need_a_store() {
        let b = Broker::new(1_000_000, 1);
        let snap = b.snapshot(UserId(42));
        assert_eq!(check_snapshot(&snap), vec![]);
    }

    #[test]
    fn real_trading_sequence_keeps_invariants() {
        let mut b = Broker::new(10_000_000, 1);
        let u = UserId(1);
        let id = b
            .open(
                u,
                &no_marks(),
                1,
                PosSide::Long,
                1000,
                50_000,
                2,
                3,
                None,
                None,
            )
            .expect("открытие");
        assert_eq!(check_snapshot(&b.snapshot(u)), vec![], "после открытия");
        b.place_pending(u, 2, PosSide::Short, 500, 40_000, 2, 3, None, None)
            .expect("ордер поставлен");
        assert_eq!(
            check_snapshot(&b.snapshot(u)),
            vec![],
            "после лимитного ордера"
        );
        b.close(u, id, 51_000).expect("закрытие");
        assert_eq!(check_snapshot(&b.snapshot(u)), vec![], "после закрытия");
    }

    #[test]
    fn used_margin_matches_sum_of_positions() {
        let mut b = Broker::new(10_000_000, 1);
        let u = UserId(1);
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            1000,
            50_000,
            2,
            3,
            None,
            None,
        )
        .unwrap();
        b.open(
            u,
            &no_marks(),
            2,
            PosSide::Short,
            300,
            20_000,
            2,
            3,
            None,
            None,
        )
        .unwrap();
        let snap = b.snapshot(u);
        let sum: Cents = snap.positions.iter().map(|p| p.margin).sum();
        assert_eq!(
            b.used_margin(u),
            sum,
            "used_margin разошёлся с суммой маржи позиций"
        );
        assert_eq!(check_snapshot(&snap), vec![]);
    }
}

#[cfg(test)]
mod netting_tests {
    use super::*;

    const PD: u8 = 2;
    const QD: u8 = 3;

    fn netting_broker() -> (Broker, UserId) {
        let mut b = Broker::new(10_000_000, 1);
        let u = UserId(1);
        assert!(
            b.adopt_model(u, PositionModel::Netting),
            "пустой счёт обязан принять модель"
        );
        (b, u)
    }

    #[test]
    fn merged_entry_is_volume_weighted() {
        assert_eq!(merged_entry(100, 5000, 300, 5400), 5300);
    }

    #[test]
    fn merged_entry_rounds_to_nearest_not_down() {
        assert_eq!(merged_entry(1, 100, 1, 101), 101);
        assert_eq!(merged_entry(1, 100, 1, 102), 101);
    }

    #[test]
    fn merged_entry_of_single_lot_is_that_price() {
        assert_eq!(merged_entry(0, 0, 50, 777), 777);
    }

    #[test]
    fn realized_pnl_uses_partial_qty_not_whole_position() {
        assert_eq!(
            realized_on_close(PosSide::Long, 5300, 5600, 150, PD, QD, 100),
            45
        );
        assert_eq!(
            realized_on_close(PosSide::Short, 5600, 5300, 150, PD, QD, 100),
            45
        );
        assert_eq!(
            realized_on_close(PosSide::Short, 5300, 5600, 150, PD, QD, 100),
            -45
        );
    }

    #[test]
    fn same_side_averages_entry_and_keeps_one_position() {
        let (mut b, u) = netting_broker();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            100,
            5000,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            300,
            5400,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();

        let ps = b.positions(u);
        assert_eq!(
            ps.len(),
            1,
            "неттинг обязан держать одну позицию на инструмент"
        );
        assert_eq!(ps[0].qty, 400);
        assert_eq!(ps[0].entry, 5300, "вход обязан быть средневзвешенным");
        assert_eq!(
            ps[0].margin, 2120,
            "маржа обязана пересчитаться на весь объём"
        );
        assert_eq!(b.used_margin(u), 2120);
    }

    #[test]
    fn adding_to_position_keeps_existing_sl_tp_when_not_given() {
        let (mut b, u) = netting_broker();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            100,
            5000,
            PD,
            QD,
            Some(4900),
            Some(5500),
        )
        .unwrap();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            100,
            5200,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        let ps = b.positions(u);
        assert_eq!(ps[0].sl, Some(4900), "SL не должен теряться при доливке");
        assert_eq!(ps[0].tp, Some(5500), "TP не должен теряться при доливке");
    }

    #[test]
    fn opposite_smaller_reduces_position_and_realizes_partial_pnl() {
        let (mut b, u) = netting_broker();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            400,
            5300,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        let before = b.balance(u).expect("счёт заведён");

        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Short,
            150,
            5600,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();

        let ps = b.positions(u);
        assert_eq!(ps.len(), 1, "позиция обязана остаться, а не удвоиться");
        assert_eq!(
            ps[0].side,
            PosSide::Long,
            "направление не меняется при частичном сворачивании"
        );
        assert_eq!(ps[0].qty, 250, "объём обязан уменьшиться на свёрнутое");
        assert_eq!(
            ps[0].entry, 5300,
            "вход не пересчитывается: свёрнута часть, а не заключена новая сделка"
        );
        assert_eq!(
            ps[0].margin, 1325,
            "маржа обязана уменьшиться пропорционально остатку"
        );
        assert_eq!(
            b.balance(u).expect("счёт заведён") - before,
            45,
            "реализован P&L только свёрнутой части"
        );

        let closed = b.closed_deals(u);
        assert_eq!(
            closed.len(),
            1,
            "в историю попадает свёрнутая часть, одной записью"
        );
        assert_eq!(closed[0].qty, 150, "в истории — фактически свёрнутый объём");
        assert_eq!(closed[0].exit, 5600);
        assert_eq!(closed[0].pnl, 45);
    }

    #[test]
    fn opposite_equal_closes_position_completely() {
        let (mut b, u) = netting_broker();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            200,
            5000,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Short,
            200,
            5100,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();

        assert!(
            b.positions(u).is_empty(),
            "встречный равный объём обязан закрыть позицию"
        );
        assert_eq!(b.used_margin(u), 0, "маржа обязана освободиться полностью");
        assert_eq!(b.closed_deals(u).len(), 1);
    }

    #[test]
    fn opposite_larger_flips_position_and_records_only_the_closed_part() {
        let (mut b, u) = netting_broker();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            100,
            5000,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        let before = b.balance(u).expect("счёт заведён");

        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Short,
            250,
            5200,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();

        let ps = b.positions(u);
        assert_eq!(ps.len(), 1, "после переворота остаётся одна позиция");
        assert_eq!(ps[0].side, PosSide::Short, "направление обязано смениться");
        assert_eq!(ps[0].qty, 150, "остаток = встречный объём минус свёрнутый");
        assert_eq!(ps[0].entry, 5200, "остаток открыт по цене встречной сделки");
        assert_eq!(ps[0].margin, 780);
        assert_eq!(
            b.balance(u).expect("счёт заведён") - before,
            20,
            "реализован P&L только закрытой части"
        );

        let closed = b.closed_deals(u);
        assert_eq!(
            closed.len(),
            1,
            "остаток ещё не закрыт — в историю он не попадает"
        );
        assert_eq!(
            closed[0].qty, 100,
            "в истории — только свёрнутый объём, не весь ордер"
        );
    }

    #[test]
    fn netting_keeps_invariants_through_a_sequence() {
        let (mut b, u) = netting_broker();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            400,
            5300,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        assert_eq!(check_snapshot(&b.snapshot(u)), vec![], "после открытия");
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Short,
            150,
            5600,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            check_snapshot(&b.snapshot(u)),
            vec![],
            "после частичного сворачивания"
        );
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Short,
            500,
            5100,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        assert_eq!(check_snapshot(&b.snapshot(u)), vec![], "после переворота");
    }

    #[test]
    fn netting_is_per_instrument() {
        let (mut b, u) = netting_broker();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            100,
            5000,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        b.open(
            u,
            &no_marks(),
            2,
            PosSide::Short,
            100,
            3000,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            b.positions(u).len(),
            2,
            "позиции по разным инструментам независимы"
        );
    }

    #[test]
    fn hedging_still_opens_a_second_position() {
        let mut b = Broker::new(10_000_000, 1);
        let u = UserId(1);
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            100,
            5000,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Short,
            100,
            5200,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            b.positions(u).len(),
            2,
            "хеджирование обязано остаться прежним"
        );
        assert!(
            b.closed_deals(u).is_empty(),
            "в хеджировании встречная сделка ничего не закрывает"
        );
    }

    #[test]
    fn default_model_is_hedging() {
        let b = Broker::new(10_000_000, 1);
        assert_eq!(
            b.model(UserId(1)),
            PositionModel::Hedging,
            "счёт без записи — хеджирующий"
        );
    }

    #[test]
    fn model_changes_only_when_flat() {
        let mut b = Broker::new(10_000_000, 1);
        let u = UserId(1);
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            100,
            5000,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        assert!(
            !b.adopt_model(u, PositionModel::Netting),
            "с открытой позицией модель менять нельзя"
        );
        assert_eq!(
            b.model(u),
            PositionModel::Hedging,
            "счёт обязан продолжить работать по прежней"
        );

        let id = b.positions(u)[0].id;
        b.close(u, id, 5100).unwrap();
        assert!(
            b.adopt_model(u, PositionModel::Netting),
            "на плоском счёте модель принимается"
        );
        assert_eq!(b.model(u), PositionModel::Netting);
    }

    #[test]
    fn pending_orders_do_not_block_model_change() {
        let mut b = Broker::new(10_000_000, 1);
        let u = UserId(1);
        b.place_pending(u, 1, PosSide::Long, 100, 4000, PD, QD, None, None)
            .expect("ордер поставлен");
        assert!(
            b.adopt_model(u, PositionModel::Netting),
            "ордер — не позиция"
        );
    }

    #[test]
    fn model_survives_snapshot_roundtrip() {
        let (mut b, u) = netting_broker();
        b.open(
            u,
            &no_marks(),
            1,
            PosSide::Long,
            100,
            5000,
            PD,
            QD,
            None,
            None,
        )
        .unwrap();
        let snap = b.snapshot(u);
        assert_eq!(snap.model, PositionModel::Netting);

        let mut restored = Broker::new(10_000_000, 1);
        restored.restore(u, snap);
        assert_eq!(
            restored.model(u),
            PositionModel::Netting,
            "модель обязана пережить рестарт"
        );
        restored
            .open(
                u,
                &no_marks(),
                1,
                PosSide::Short,
                100,
                5100,
                PD,
                QD,
                None,
                None,
            )
            .unwrap();
        assert!(
            restored.positions(u).is_empty(),
            "восстановленный счёт обязан неттить"
        );
    }
}

#[cfg(test)]
mod account_leverage_tests {
    use super::journal::{self, Journal, Origin};
    use super::*;

    const U: UserId = UserId(7);

    fn setup(platform_lev: i128) -> (Broker, Journal) {
        let mut b = Broker::new(1_000_000_000, platform_lev);
        assert!(b.open_account(U));
        let mut j = Journal::new(b.start_balance());
        commit(&mut b, &mut j);
        (b, j)
    }

    fn commit(b: &mut Broker, j: &mut Journal) {
        let kinds = b.take_events(U);
        let sealed = j.seal(1, &Origin::System, &kinds);
        let snap = b.snapshot(U);
        assert_eq!(
            journal::reconcile(&j.preview(&sealed), &snap),
            vec![],
            "лента сходится со слепком"
        );
        for e in sealed {
            j.commit(e).expect("запись продолжает ленту");
        }
    }

    fn marks(px: i64) -> HashMap<u32, i64> {
        HashMap::from([(1u32, px)])
    }

    fn lev_of(b: &Broker, id: u64) -> i128 {
        b.positions(U)
            .into_iter()
            .find(|p| p.id == id)
            .expect("позиция есть")
            .leverage
    }

    #[test]
    fn unassigned_account_trades_at_platform_leverage() {
        let (mut b, mut j) = setup(20);
        assert_eq!(b.assigned_leverage(U), None);
        assert_eq!(b.account_leverage(U), 20, "умолчание — плечо площадки");
        let id = b
            .open(
                U,
                &marks(5000),
                1,
                PosSide::Long,
                1000,
                5000,
                2,
                3,
                None,
                None,
            )
            .unwrap();
        commit(&mut b, &mut j);
        assert_eq!(lev_of(&b, id), 20);
    }

    #[test]
    fn new_position_takes_new_leverage_old_keeps_its_own_and_survives_restart() {
        let (mut b, mut j) = setup(20);
        let old = b
            .open(
                U,
                &marks(5000),
                1,
                PosSide::Long,
                1000,
                5000,
                2,
                3,
                None,
                None,
            )
            .unwrap();
        commit(&mut b, &mut j);
        let old_margin = b.used_margin(U);

        assert_eq!(b.set_account_leverage(U, 50), Ok(true));
        assert_eq!(
            b.take_events_peek(),
            vec![EventKind::LeverageSet { lev: 50 }],
            "смена плеча — своя запись"
        );
        commit(&mut b, &mut j);
        assert_eq!(
            b.used_margin(U),
            old_margin,
            "маржа открытой позиции не двинулась"
        );

        let new = b
            .open(
                U,
                &marks(5000),
                1,
                PosSide::Long,
                1000,
                5000,
                2,
                3,
                None,
                None,
            )
            .unwrap();
        commit(&mut b, &mut j);
        assert_eq!(lev_of(&b, old), 20, "старая позиция — со старым плечом");
        assert_eq!(lev_of(&b, new), 50, "новая — с плечом счёта");

        let events = j.events().to_vec();
        let reloaded = Journal::load(b.start_balance(), events).expect("лента цела");
        assert_eq!(reloaded.state().leverage, Some(50), "плечо в свёртке");
        let snap = b.snapshot(U);
        assert_eq!(snap.leverage, Some(50), "плечо в слепке");
        assert!(
            journal::audit(&reloaded, &snap).ok(),
            "сверка после рестарта"
        );
        let mut b2 = Broker::new(b.start_balance(), 20);
        b2.restore(U, snap);
        assert_eq!(
            b2.account_leverage(U),
            50,
            "после подъёма плечо счёта прежнее"
        );
    }

    #[test]
    fn leverage_without_a_journal_entry_is_a_divergence() {
        let (mut b, j) = setup(20);
        b.set_account_leverage(U, 50).unwrap();
        let _ = b.take_events(U);
        let ds = journal::reconcile(j.state(), &b.snapshot(U));
        assert!(
            ds.iter().any(|d| d.0.contains("плечо счёта")),
            "расхождение названо: {ds:?}"
        );
    }

    #[test]
    fn leverage_is_validated_and_a_repeat_writes_nothing() {
        let (mut b, _j) = setup(20);
        for bad in [0, -1, MAX_LEVERAGE + 1] {
            assert_eq!(
                b.set_account_leverage(U, bad),
                Err(BrokerError::InvalidLeverage),
                "плечо {bad} не принимается"
            );
        }
        assert_eq!(b.set_account_leverage(U, MAX_LEVERAGE), Ok(true));
        let _ = b.take_events(U);
        assert_eq!(b.set_account_leverage(U, MAX_LEVERAGE), Ok(false));
        assert_eq!(b.pending_events(U), 0, "повтор не пишет запись");
        assert_eq!(
            b.set_account_leverage(UserId(999), 10),
            Err(BrokerError::UnknownAccount),
            "счёт не заводится ради плеча"
        );
    }

    #[test]
    fn pending_order_executes_at_the_leverage_in_force_when_it_fires() {
        let (mut b, mut j) = setup(20);
        b.place_pending(U, 1, PosSide::Long, 1000, 4900, 2, 3, None, None)
            .unwrap();
        commit(&mut b, &mut j);
        b.set_account_leverage(U, 5).unwrap();
        commit(&mut b, &mut j);

        assert_eq!(b.check(&marks(4890), false), vec![U]);
        commit(&mut b, &mut j);
        let p = b.positions(U);
        assert_eq!(p.len(), 1, "ордер исполнен");
        assert_eq!(p[0].leverage, 5, "плечо — действующее при исполнении");
    }

    #[test]
    fn frozen_account_pending_is_rejected_on_fire_not_executed() {
        let (mut b, mut j) = setup(20);
        let id = b
            .place_pending(U, 1, PosSide::Long, 1000, 4900, 2, 3, None, None)
            .unwrap();
        commit(&mut b, &mut j);

        let frozen = HashSet::from([U]);
        assert_eq!(b.check_with_frozen(&marks(4890), false, &frozen), vec![U]);
        assert_eq!(
            b.take_events_peek(),
            vec![EventKind::PendingRejected {
                id,
                exec: Some(4890),
                reason: RejectReason::AccountFrozen,
            }]
        );
        commit(&mut b, &mut j);
        assert!(b.positions(U).is_empty(), "позиция не открыта");
        assert!(b.pendings(U).is_empty(), "ордер снят");
        assert!(j.state().pendings.is_empty(), "и в свёртке тоже");
    }

    #[test]
    fn frozen_account_pending_that_does_not_fire_stays() {
        let (mut b, mut j) = setup(20);
        b.place_pending(U, 1, PosSide::Long, 1000, 4900, 2, 3, None, None)
            .unwrap();
        commit(&mut b, &mut j);
        let frozen = HashSet::from([U]);
        assert!(b.check_with_frozen(&marks(5000), false, &frozen).is_empty());
        assert_eq!(b.pendings(U).len(), 1);
    }

    impl Broker {
        fn take_events_peek(&self) -> Vec<EventKind> {
            self.accounts
                .get(&U)
                .map(|a| a.outbox.clone())
                .unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod fees_tests {
    use super::journal::{self, Journal, Origin};
    use super::*;

    const U: UserId = UserId(9);
    const PX: i64 = 5_000_000;
    const Q: i64 = 1000;

    fn setup() -> (Broker, Journal) {
        setup_with(Broker::new(1_000_000_000, 20))
    }

    fn setup_with(mut b: Broker) -> (Broker, Journal) {
        assert!(b.open_account(U));
        let mut j = Journal::new(b.start_balance());
        commit(&mut b, &mut j);
        (b, j)
    }

    fn commit(b: &mut Broker, j: &mut Journal) -> Vec<EventKind> {
        let kinds = b.take_events(U);
        let sealed = j.seal(1, &Origin::System, &kinds);
        let snap = b.snapshot(U);
        assert_eq!(
            journal::reconcile(&j.preview(&sealed), &snap),
            vec![],
            "лента сходится со слепком"
        );
        for e in sealed {
            j.commit(e).expect("запись продолжает ленту");
        }
        kinds
    }

    fn marks(px: i64) -> HashMap<u32, i64> {
        HashMap::from([(1u32, px)])
    }

    fn fees(markup_bps: i64, commission_bps: i64) -> Fees {
        Fees {
            markup_bps,
            commission_bps,
        }
    }

    fn open(b: &mut Broker, side: PosSide, market: i64) -> Result<u64, BrokerError> {
        b.open(U, &marks(market), 1, side, Q, market, 2, 3, None, None)
    }

    fn pos(b: &Broker, id: u64) -> Position {
        b.positions(U).into_iter().find(|p| p.id == id).unwrap()
    }

    #[test]
    fn helpers_round_to_nearest() {
        assert_eq!(apply_markup(true, PX, 10), PX + 5000);
        assert_eq!(apply_markup(false, PX, 10), PX - 5000);
        assert_eq!(apply_markup(true, PX, 0), PX, "ноль — рынок");
        assert_eq!(apply_markup(true, 15, 1), 15);
        assert_eq!(apply_markup(true, 5000, 1), 5001);
        assert_eq!(commission_for(5_005_000, Q, 2, 3, 7, 100), 3504);
        assert_eq!(commission_for(PX, Q, 2, 3, 0, 100), 0);
        assert_eq!(markup_amount(PX, PX + 5000, Q, 2, 3, 100), 5000);
        assert_eq!(markup_amount(PX, PX - 5000, Q, 2, 3, 100), 5000);
    }

    #[test]
    fn markup_moves_the_price_against_the_client_on_both_sides() {
        let (mut b, mut j) = setup();
        b.set_account_fees(U, fees(10, 0)).unwrap();
        commit(&mut b, &mut j);

        let long = open(&mut b, PosSide::Long, PX).unwrap();
        commit(&mut b, &mut j);
        assert_eq!(pos(&b, long).entry, PX + 5000, "покупка дороже ask");
        let short = open(&mut b, PosSide::Short, PX).unwrap();
        commit(&mut b, &mut j);
        assert_eq!(pos(&b, short).entry, PX - 5000, "продажа дешевле bid");

        b.close(U, long, PX).unwrap();
        commit(&mut b, &mut j);
        b.close(U, short, PX).unwrap();
        let kinds = commit(&mut b, &mut j);
        let deals = b.closed_deals(U);
        assert_eq!(deals[0].exit, PX - 5000, "закрытие лонга — продажа дешевле");
        assert_eq!(deals[1].exit, PX + 5000, "закрытие шорта — покупка дороже");
        for d in &deals {
            assert_eq!(
                d.pnl, -10_000,
                "рынок не двигался — клиент теряет обе наценки"
            );
            assert_eq!(d.fees.markup, 10_000, "наценка входа и выхода в сделке");
            assert_eq!(d.fees.commission, 0);
            assert_eq!(d.fees.market_exit, Some(PX), "рыночная цена закрытия");
        }
        assert!(kinds.contains(&EventKind::MarkupApplied {
            trade: short,
            mkt: PX,
            exec: PX + 5000,
            amount: 5000
        }));
        assert_eq!(b.balance(U), Some(1_000_000_000 - 20_000));
    }

    #[test]
    fn commission_is_charged_on_open_and_on_close() {
        let (mut b, mut j) = setup();
        b.set_account_fees(U, fees(0, 7)).unwrap();
        assert_eq!(
            commit(&mut b, &mut j),
            vec![EventKind::FeesSet {
                markup_bps: 0,
                commission_bps: 7
            }]
        );
        let id = open(&mut b, PosSide::Long, PX).unwrap();
        let kinds = commit(&mut b, &mut j);
        assert_eq!(
            kinds.last(),
            Some(&EventKind::CommissionCharged {
                amount: 3500,
                trade: id,
                waived: 0
            })
        );
        assert_eq!(b.balance(U), Some(1_000_000_000 - 3500));
        assert_eq!(pos(&b, id).fees.commission, 3500);

        b.close(U, id, 5_100_000).unwrap();
        let kinds = commit(&mut b, &mut j);
        assert!(matches!(
            kinds.as_slice(),
            [
                EventKind::CommissionCharged {
                    amount: 3570,
                    waived: 0,
                    ..
                },
                EventKind::PositionClosed { .. }
            ]
        ));
        let d = &b.closed_deals(U)[0];
        assert_eq!(d.pnl, 100_000, "результат — по цене, без комиссии");
        assert_eq!(d.fees.commission, 3500 + 3570, "комиссия входа и выхода");
        assert_eq!(b.balance(U), Some(1_000_000_000 + 100_000 - 7070));
    }

    #[test]
    fn admission_counts_the_commission() {
        let (mut b, _j) = setup_with(Broker::new(251_000, 20));
        b.set_account_fees(U, fees(0, 5)).unwrap();
        assert_eq!(
            open(&mut b, PosSide::Long, PX),
            Err(BrokerError::InsufficientMargin)
        );
        b.set_account_fees(U, fees(0, 1)).unwrap();
        assert!(open(&mut b, PosSide::Long, PX).is_ok());
    }

    #[test]
    fn stop_out_pays_commission() {
        let (mut b, mut j) = setup_with(Broker::new(300_000, 20).with_stop_out_pct(Some(50)));
        b.set_account_fees(U, fees(0, 10)).unwrap();
        commit(&mut b, &mut j);
        let id = open(&mut b, PosSide::Long, PX).unwrap();
        commit(&mut b, &mut j);
        let px = 4_800_000;
        assert_eq!(b.check(&marks(px), false), vec![U]);
        let kinds = commit(&mut b, &mut j);
        assert!(b.positions(U).is_empty(), "закрыта стоп-аутом");
        assert!(
            kinds.contains(&EventKind::CommissionCharged {
                amount: commission_for(px, Q, 2, 3, 10, 100),
                trade: id,
                waived: 0
            }),
            "{kinds:?}"
        );
        assert!(kinds.iter().any(|k| matches!(
            k,
            EventKind::PositionClosed {
                cause: CloseCause::StopOut,
                ..
            }
        )));
    }

    #[test]
    fn stop_loss_pays_commission() {
        let (mut b, mut j) = setup();
        b.set_account_fees(U, fees(0, 10)).unwrap();
        commit(&mut b, &mut j);
        let id = b
            .open(
                U,
                &marks(PX),
                1,
                PosSide::Long,
                Q,
                PX,
                2,
                3,
                Some(4_900_000),
                None,
            )
            .unwrap();
        commit(&mut b, &mut j);
        b.check(&marks(4_890_000), false);
        let kinds = commit(&mut b, &mut j);
        assert!(kinds.contains(&EventKind::CommissionCharged {
            amount: commission_for(4_890_000, Q, 2, 3, 10, 100),
            trade: id,
            waived: 0
        }));
        assert_eq!(b.closed_deals(U).len(), 1);
    }

    #[test]
    fn pending_order_pays_at_the_tariff_in_force_when_it_fires() {
        let (mut b, mut j) = setup();
        b.place_pending(U, 1, PosSide::Long, Q, 4_900_000, 2, 3, None, None)
            .unwrap();
        commit(&mut b, &mut j);
        b.set_account_fees(U, fees(10, 3)).unwrap();
        commit(&mut b, &mut j);
        b.check(&marks(4_890_000), false);
        let kinds = commit(&mut b, &mut j);
        let p = b.positions(U)[0].clone();
        assert_eq!(p.entry, 4_890_000 + 4890, "наценка на исполнении ордера");
        assert_eq!(p.fees.rates(), fees(10, 3));
        assert!(kinds
            .iter()
            .any(|k| matches!(k, EventKind::CommissionCharged { trade, .. } if *trade == p.id)));
    }

    #[test]
    fn open_positions_keep_their_tariff_and_everything_survives_a_restart() {
        let (mut b, mut j) = setup();
        b.set_account_fees(U, fees(10, 5)).unwrap();
        commit(&mut b, &mut j);
        let old = open(&mut b, PosSide::Long, PX).unwrap();
        commit(&mut b, &mut j);
        b.set_account_fees(U, fees(0, 0)).unwrap();
        commit(&mut b, &mut j);
        let new = open(&mut b, PosSide::Long, PX).unwrap();
        commit(&mut b, &mut j);
        assert_eq!(
            pos(&b, old).fees.rates(),
            fees(10, 5),
            "старая — свой тариф"
        );
        assert_eq!(pos(&b, new).fees.rates(), fees(0, 0), "новая — тариф счёта");
        assert_eq!(pos(&b, new).entry, PX, "без наценки");

        let snap = b.snapshot(U);
        let reloaded = Journal::load(b.start_balance(), j.events().to_vec()).unwrap();
        assert!(
            journal::audit(&reloaded, &snap).ok(),
            "сверка после рестарта"
        );
        assert_eq!(reloaded.state().fees, fees(0, 0));
        let mut b2 = Broker::new(b.start_balance(), 20);
        b2.restore(U, snap);
        let mut j2 = reloaded;

        b2.close(U, old, PX).unwrap();
        commit(&mut b2, &mut j2);
        b2.close(U, new, PX).unwrap();
        commit(&mut b2, &mut j2);
        let d = b2.closed_deals(U);
        assert_eq!(
            d[0].exit,
            PX - 5000,
            "старая закрыта с наценкой своего тарифа"
        );
        assert!(d[0].fees.commission > 0);
        assert_eq!(d[1].exit, PX, "новая — без наценки");
        assert_eq!(
            d[1].fees,
            DealFees {
                commission: 0,
                markup: 0,
                market_exit: Some(PX)
            }
        );
        assert!(journal::audit(&j2, &b2.snapshot(U)).ok());
        let replayed = j2.replay();
        assert_eq!(replayed.closed[0].fees.market_exit, Some(PX));
        assert_eq!(replayed.closed[0].fees.markup, d[0].fees.markup);
    }

    #[test]
    fn netting_reduce_increase_and_flip_reconcile() {
        let (mut b, mut j) = setup();
        assert!(b.adopt_model(U, PositionModel::Netting));
        b.set_account_fees(U, fees(10, 5)).unwrap();
        commit(&mut b, &mut j);
        let id = open(&mut b, PosSide::Long, PX).unwrap();
        commit(&mut b, &mut j);
        b.set_account_fees(U, fees(20, 1)).unwrap();
        commit(&mut b, &mut j);
        open(&mut b, PosSide::Long, PX).unwrap();
        commit(&mut b, &mut j);
        assert_eq!(pos(&b, id).entry, PX + 5000);
        b.open(U, &marks(PX), 1, PosSide::Short, 500, PX, 2, 3, None, None)
            .unwrap();
        commit(&mut b, &mut j);
        assert_eq!(b.closed_deals(U)[0].exit, PX - 5000, "по снимку позиции");
        assert!(b.closed_deals(U)[0].fees.commission > 0);
        assert_eq!(pos(&b, id).fees.commission, 0, "накопленное ушло в сделку");
        let new = b
            .open(U, &marks(PX), 1, PosSide::Short, 2000, PX, 2, 3, None, None)
            .unwrap();
        commit(&mut b, &mut j);
        assert_ne!(new, id);
        assert_eq!(pos(&b, new).entry, PX - 10_000, "20 bps — тариф счёта");
        assert_eq!(pos(&b, new).fees.rates(), fees(20, 1));
        assert!(journal::audit(&j, &b.snapshot(U)).ok());
    }

    #[test]
    fn negative_balance_protection_caps_the_commission() {
        let (mut b, mut j) =
            setup_with(Broker::new(300_000, 20).with_negative_balance_protection(true));
        b.set_account_fees(U, fees(0, 10)).unwrap();
        commit(&mut b, &mut j);
        let id = open(&mut b, PosSide::Long, PX).unwrap();
        commit(&mut b, &mut j);
        let before = b.balance(U).unwrap();
        b.close(U, id, 4_500_000).unwrap();
        b.check(&marks(4_500_000), false);
        let kinds = commit(&mut b, &mut j);
        let due = commission_for(4_500_000, Q, 2, 3, 10, 100);
        assert!(
            kinds.contains(&EventKind::CommissionCharged {
                amount: 0,
                trade: id,
                waived: due
            }),
            "{kinds:?}"
        );
        assert_eq!(b.balance(U), Some(0), "защита погасила минус");
        assert!(kinds.contains(&EventKind::NegativeBalanceCompensated {
            amount: 500_000 - before
        }));
    }

    #[test]
    fn without_protection_the_commission_is_charged_in_full() {
        let (mut b, mut j) = setup_with(Broker::new(300_000, 20));
        b.set_account_fees(U, fees(0, 10)).unwrap();
        commit(&mut b, &mut j);
        let id = open(&mut b, PosSide::Long, PX).unwrap();
        commit(&mut b, &mut j);
        b.close(U, id, 4_500_000).unwrap();
        let kinds = commit(&mut b, &mut j);
        let due = commission_for(4_500_000, Q, 2, 3, 10, 100);
        assert!(kinds.contains(&EventKind::CommissionCharged {
            amount: due,
            trade: id,
            waived: 0
        }));
        assert!(b.balance(U).unwrap() < 0);
    }

    #[test]
    fn fees_are_validated_journaled_and_a_repeat_writes_nothing() {
        let (mut b, j) = setup();
        let mut j = j;
        for bad in [
            fees(-1, 0),
            fees(0, -1),
            fees(MAX_FEE_BPS + 1, 0),
            fees(0, 501),
        ] {
            assert_eq!(b.set_account_fees(U, bad), Err(BrokerError::InvalidFees));
        }
        assert_eq!(
            b.set_account_fees(U, fees(MAX_FEE_BPS, MAX_FEE_BPS)),
            Ok(true)
        );
        commit(&mut b, &mut j);
        assert_eq!(
            b.set_account_fees(U, fees(MAX_FEE_BPS, MAX_FEE_BPS)),
            Ok(false)
        );
        assert_eq!(b.pending_events(U), 0);
        assert_eq!(
            b.set_account_fees(UserId(999), fees(1, 1)),
            Err(BrokerError::UnknownAccount)
        );
        b.set_account_fees(U, fees(1, 1)).unwrap();
        let _ = b.take_events(U);
        let ds = journal::reconcile(j.state(), &b.snapshot(U));
        assert!(ds.iter().any(|d| d.0.contains("тариф счёта")), "{ds:?}");
    }

    #[test]
    fn a_tariff_out_of_range_in_storage_is_a_violation() {
        let (mut b, _j) = setup();
        open(&mut b, PosSide::Long, PX).unwrap();
        assert_eq!(check_snapshot(&b.snapshot(U)), vec![]);
        let mut snap = b.snapshot(U);
        snap.fees = fees(MAX_FEE_BPS + 1, 0);
        snap.positions[0].fees.commission_bps = -1;
        snap.positions[0].fees.markup = -5;
        let v = check_snapshot(&snap);
        assert_eq!(v.len(), 3, "{v:?}");
    }

    #[test]
    fn zero_tariff_is_the_old_behaviour() {
        let (mut b, mut j) = setup();
        let id = open(&mut b, PosSide::Long, PX).unwrap();
        let kinds = commit(&mut b, &mut j);
        assert_eq!(kinds.len(), 1, "только PositionOpened: {kinds:?}");
        assert_eq!(pos(&b, id).entry, PX);
        b.close(U, id, PX).unwrap();
        let kinds = commit(&mut b, &mut j);
        assert_eq!(kinds.len(), 1, "только PositionClosed: {kinds:?}");
        assert_eq!(b.balance(U), Some(1_000_000_000));
    }

    #[test]
    fn fee_records_round_trip_and_garbage_is_refused() {
        for k in [
            EventKind::FeesSet {
                markup_bps: 15,
                commission_bps: 0,
            },
            EventKind::MarkupApplied {
                trade: 3,
                mkt: 100,
                exec: 101,
                amount: 7,
            },
            EventKind::CommissionCharged {
                amount: 5,
                trade: 3,
                waived: 0,
            },
            EventKind::CommissionCharged {
                amount: 0,
                trade: 3,
                waived: 9,
            },
        ] {
            assert_eq!(
                EventKind::decode(&k.encode()),
                Ok(k.clone()),
                "{}",
                k.encode()
            );
        }
        assert_eq!(
            EventKind::CommissionCharged {
                amount: 5,
                trade: 3,
                waived: 0
            }
            .encode(),
            "commission_charged amount=5 trade=3"
        );
        for bad in [
            "fees_set markup_bps=501 commission_bps=0",
            "fees_set markup_bps=-1 commission_bps=0",
            "commission_charged amount=-1 trade=3",
            "markup_applied trade=1 mkt=1 exec=2 amount=-3",
        ] {
            assert!(EventKind::decode(bad).is_err(), "{bad}");
        }
    }
}

#[cfg(test)]
mod levels_tests {
    use super::journal::{self, CloseCause, EventKind, Journal, Origin};
    use super::*;

    const U: UserId = UserId(11);
    const PX: i64 = 5_000_000;

    fn setup() -> (Broker, Journal) {
        let mut b = Broker::new(1_000_000_000, 10);
        assert!(b.open_account(U));
        let mut j = Journal::new(b.start_balance());
        commit(&mut b, &mut j);
        (b, j)
    }

    fn commit(b: &mut Broker, j: &mut Journal) -> Vec<EventKind> {
        let kinds = b.take_events(U);
        let sealed = j.seal(1, &Origin::Client, &kinds);
        assert_eq!(
            journal::reconcile(&j.preview(&sealed), &b.snapshot(U)),
            vec![],
            "лента сходится со слепком"
        );
        for e in sealed {
            j.commit(e).unwrap();
        }
        kinds
    }

    fn marks(px: i64) -> HashMap<u32, i64> {
        HashMap::from([(1u32, px)])
    }

    fn pos(b: &Broker, id: u64) -> Position {
        b.positions(U).into_iter().find(|p| p.id == id).unwrap()
    }

    fn open(b: &mut Broker, side: PosSide, sl: Option<i64>) -> u64 {
        b.open(U, &marks(PX), 1, side, 1000, PX, 2, 3, sl, None)
            .unwrap()
    }

    #[test]
    fn set_move_and_remove_levels_are_journalled_and_replayed() {
        let (mut b, mut j) = setup();
        let id = open(&mut b, PosSide::Long, None);
        commit(&mut b, &mut j);

        assert_eq!(
            b.set_position_levels(U, id, Some(4_900_000), Some(5_200_000), &[PX]),
            Ok(true)
        );
        assert_eq!(
            commit(&mut b, &mut j),
            vec![EventKind::PositionLevelsChanged {
                id,
                sl: Some(4_900_000),
                tp: Some(5_200_000)
            }]
        );
        assert_eq!(
            b.set_position_levels(U, id, Some(4_950_000), Some(5_200_000), &[PX]),
            Ok(true)
        );
        commit(&mut b, &mut j);
        assert_eq!(
            b.set_position_levels(U, id, Some(4_950_000), None, &[PX]),
            Ok(true)
        );
        commit(&mut b, &mut j);
        assert_eq!((pos(&b, id).sl, pos(&b, id).tp), (Some(4_950_000), None));

        assert_eq!(
            b.set_position_levels(U, id, Some(4_950_000), None, &[PX]),
            Ok(false)
        );
        assert!(commit(&mut b, &mut j).is_empty());

        let replayed = j.replay();
        assert_eq!(replayed.positions[0].sl, Some(4_950_000));
        assert_eq!(replayed.positions[0].tp, None);
        assert!(journal::audit(&j, &b.snapshot(U)).ok());
    }

    #[test]
    fn a_level_already_reached_is_refused_and_leaves_no_trace() {
        let (mut b, mut j) = setup();
        let long = open(&mut b, PosSide::Long, None);
        let short = open(&mut b, PosSide::Short, None);
        commit(&mut b, &mut j);
        assert_eq!(
            b.set_position_levels(U, long, Some(PX + 1), None, &[PX]),
            Err(BrokerError::StopLossAlreadyReached)
        );
        assert_eq!(
            b.set_position_levels(U, long, None, Some(PX - 1), &[PX]),
            Err(BrokerError::TakeProfitAlreadyReached)
        );
        assert_eq!(
            b.set_position_levels(U, short, Some(PX - 1), None, &[PX]),
            Err(BrokerError::StopLossAlreadyReached)
        );
        assert_eq!(
            b.set_position_levels(U, long, Some(PX + 5), None, &[PX + 10, PX]),
            Err(BrokerError::StopLossAlreadyReached)
        );
        assert!(commit(&mut b, &mut j).is_empty(), "отказ не пишет");
        assert_eq!(pos(&b, long).sl, None);
    }

    #[test]
    fn only_the_changed_level_is_checked() {
        let (mut b, mut j) = setup();
        let id = open(&mut b, PosSide::Long, Some(PX - 100));
        commit(&mut b, &mut j);
        assert_eq!(
            b.set_position_levels(U, id, Some(PX - 100), Some(PX + 500), &[PX - 100]),
            Ok(true)
        );
        commit(&mut b, &mut j);
    }

    #[test]
    fn unknown_position_and_closed_position_are_refused() {
        let (mut b, mut j) = setup();
        let id = open(&mut b, PosSide::Long, Some(PX - 10));
        commit(&mut b, &mut j);
        b.check(&marks(PX - 20), false);
        commit(&mut b, &mut j);
        assert_eq!(
            b.set_position_levels(U, id, Some(PX - 1000), None, &[PX]),
            Err(BrokerError::UnknownPosition)
        );
        assert_eq!(
            b.set_position_levels(UserId(999), 1, None, None, &[PX]),
            Err(BrokerError::UnknownPosition)
        );
        assert!(commit(&mut b, &mut j).is_empty());
    }

    #[test]
    fn the_stop_engine_uses_the_new_level_on_the_next_tick() {
        let (mut b, mut j) = setup();
        let id = open(&mut b, PosSide::Long, Some(PX - 1000));
        commit(&mut b, &mut j);
        b.set_position_levels(U, id, Some(PX - 100), None, &[PX])
            .unwrap();
        commit(&mut b, &mut j);
        assert_eq!(b.check(&marks(PX - 200), false), vec![U]);
        let kinds = commit(&mut b, &mut j);
        assert!(
            kinds.iter().any(|k| matches!(
                k,
                EventKind::PositionClosed {
                    level: Some(l),
                    cause: CloseCause::StopLoss,
                    ..
                } if *l == PX - 100
            )),
            "{kinds:?}"
        );
    }

    #[test]
    fn pending_levels_are_checked_against_the_order_price_and_ride_into_the_position() {
        let (mut b, mut j) = setup();
        let id = b
            .place_pending(U, 1, PosSide::Long, 1000, PX - 1000, 2, 3, None, None)
            .unwrap();
        commit(&mut b, &mut j);
        assert_eq!(
            b.set_pending_levels(U, id, Some(PX - 900), None),
            Err(BrokerError::StopLossAlreadyReached)
        );
        assert_eq!(
            b.set_pending_levels(U, id, Some(PX - 2000), Some(PX)),
            Ok(true)
        );
        assert_eq!(
            commit(&mut b, &mut j),
            vec![EventKind::PendingLevelsChanged {
                id,
                sl: Some(PX - 2000),
                tp: Some(PX)
            }]
        );
        assert_eq!(
            b.set_pending_levels(U, id, Some(PX - 2000), Some(PX)),
            Ok(false)
        );
        assert_eq!(
            b.set_pending_levels(U, 77, None, None),
            Err(BrokerError::UnknownPending)
        );
        b.check(&marks(PX - 1000), false);
        commit(&mut b, &mut j);
        let p = &b.positions(U)[0];
        assert_eq!((p.sl, p.tp), (Some(PX - 2000), Some(PX)));
        assert!(journal::audit(&j, &b.snapshot(U)).ok());
    }

    #[test]
    fn level_records_round_trip_and_garbage_is_refused() {
        for k in [
            EventKind::PositionLevelsChanged {
                id: 4,
                sl: Some(100),
                tp: None,
            },
            EventKind::PositionLevelsChanged {
                id: 4,
                sl: None,
                tp: None,
            },
            EventKind::PendingLevelsChanged {
                id: 9,
                sl: None,
                tp: Some(250),
            },
        ] {
            assert_eq!(EventKind::decode(&k.encode()), Ok(k.clone()));
        }
        assert_eq!(
            EventKind::PositionLevelsChanged {
                id: 4,
                sl: Some(100),
                tp: None
            }
            .encode(),
            "position_levels_changed id=4 sl=100 tp=-"
        );
        for bad in [
            "position_levels_changed id=4 sl=0 tp=-",
            "position_levels_changed id=4 sl=-5 tp=-",
            "pending_levels_changed id=4 sl=x tp=-",
            "position_levels_changed id=4 sl=1",
        ] {
            assert!(EventKind::decode(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn pending_level_divergence_is_reported() {
        let (mut b, mut j) = setup();
        b.place_pending(U, 1, PosSide::Long, 1000, PX, 2, 3, None, None)
            .unwrap();
        commit(&mut b, &mut j);
        let mut snap = b.snapshot(U);
        snap.pendings[0].sl = Some(1);
        let ds = journal::reconcile(&j.replay(), &snap);
        assert!(ds.iter().any(|d| d.0.contains("SL/TP")), "{ds:?}");
    }
}
