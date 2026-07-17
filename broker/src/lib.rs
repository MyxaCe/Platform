//! # broker
//!
//! Бумажный брокер (ADR-014): счета с виртуальным балансом, позиции Long/Short, маржа и P&L на
//! реальных ценах. Деньги — в **центах** (`i128`). Реальных денег нет.

use std::collections::HashMap;

use domain::account::UserId;

/// Деньги в центах (1/100 USD).
pub type Cents = i128;

/// Сторона позиции.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosSide {
    Long,
    Short,
}

/// Стоимость `price · qty` в центах: `price_raw · qty_raw · 100 / 10^(pd+qd)`.
/// `price_raw` может быть отрицательным (для дельты цены при расчёте PnL).
fn value_cents(price_raw: i64, qty_raw: i64, pd: u8, qd: u8) -> Cents {
    price_raw as i128 * qty_raw as i128 * 100 / 10i128.pow(pd as u32 + qd as u32)
}

/// Открытая позиция.
#[derive(Debug, Clone)]
pub struct Position {
    pub id: u64,
    pub instrument: u32,
    pub side: PosSide,
    pub qty: i64,   // raw
    pub entry: i64, // raw price
    pub pd: u8,
    pub qd: u8,
    pub margin: Cents,
}

impl Position {
    /// Нереализованный P&L при марк-цене `mark_raw`.
    pub fn unrealized(&self, mark_raw: i64) -> Cents {
        let diff = match self.side {
            PosSide::Long => mark_raw - self.entry,
            PosSide::Short => self.entry - mark_raw,
        };
        value_cents(diff, self.qty, self.pd, self.qd)
    }
}

#[derive(Debug, Clone)]
struct Account {
    balance: Cents,
    next_pos: u64,
    positions: HashMap<u64, Position>,
}

/// Ошибка операции брокера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerError {
    /// Недостаточно свободной маржи для открытия.
    InsufficientMargin,
    /// Позиции с таким id нет.
    UnknownPosition,
}

pub struct Broker {
    start_balance: Cents,
    leverage: i128,
    accounts: HashMap<UserId, Account>,
}

impl Broker {
    pub fn new(start_balance: Cents, leverage: i128) -> Self {
        Self { start_balance, leverage: leverage.max(1), accounts: HashMap::new() }
    }

    fn acct(&mut self, user: UserId) -> &mut Account {
        let start = self.start_balance;
        self.accounts.entry(user).or_insert_with(|| Account { balance: start, next_pos: 0, positions: HashMap::new() })
    }

    // ---- Чтение -----------------------------------------------------------

    pub fn balance(&self, user: UserId) -> Cents {
        self.accounts.get(&user).map(|a| a.balance).unwrap_or(self.start_balance)
    }

    pub fn used_margin(&self, user: UserId) -> Cents {
        self.accounts.get(&user).map(|a| a.positions.values().map(|p| p.margin).sum()).unwrap_or(0)
    }

    pub fn free_margin(&self, user: UserId) -> Cents {
        self.balance(user) - self.used_margin(user)
    }

    pub fn positions(&self, user: UserId) -> Vec<Position> {
        self.accounts.get(&user).map(|a| a.positions.values().cloned().collect()).unwrap_or_default()
    }

    /// Суммарный нереализованный P&L при заданных марк-ценах (инструмент → raw цена).
    pub fn open_pnl(&self, user: UserId, marks: &HashMap<u32, i64>) -> Cents {
        self.accounts
            .get(&user)
            .map(|a| a.positions.values().map(|p| p.unrealized(*marks.get(&p.instrument).unwrap_or(&p.entry))).sum())
            .unwrap_or(0)
    }

    /// Equity = баланс + нереализованный P&L.
    pub fn equity(&self, user: UserId, marks: &HashMap<u32, i64>) -> Cents {
        self.balance(user) + self.open_pnl(user, marks)
    }

    // ---- Операции ---------------------------------------------------------

    /// Открыть позицию по цене входа `entry`. Возвращает id позиции.
    #[allow(clippy::too_many_arguments)]
    pub fn open(&mut self, user: UserId, instrument: u32, side: PosSide, qty: i64, entry: i64, pd: u8, qd: u8) -> Result<u64, BrokerError> {
        let notional = value_cents(entry, qty, pd, qd);
        let margin = notional / self.leverage;
        if self.free_margin(user) < margin {
            return Err(BrokerError::InsufficientMargin);
        }
        let a = self.acct(user);
        a.next_pos += 1;
        let id = a.next_pos;
        a.positions.insert(id, Position { id, instrument, side, qty, entry, pd, qd, margin });
        Ok(id)
    }

    /// Закрыть позицию по марк-цене `mark`. Возвращает реализованный P&L (зачислен в баланс).
    pub fn close(&mut self, user: UserId, id: u64, mark: i64) -> Result<Cents, BrokerError> {
        let a = self.acct(user);
        let pos = a.positions.remove(&id).ok_or(BrokerError::UnknownPosition)?;
        let pnl = pos.unrealized(mark);
        a.balance += pnl;
        Ok(pnl)
    }
}
