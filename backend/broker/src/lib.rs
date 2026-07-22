//! # broker
//!
//! Бумажный брокер (ADR-014): счета с виртуальным балансом, позиции Long/Short с SL/TP, отложенные
//! (лимитные) ордера с триггером по цене, история закрытых сделок, маржа и P&L на реальных ценах.
//! Деньги — в **центах** (`i128`). Реальных денег нет.

use std::collections::HashMap;

use domain::account::UserId;

/// Деньги в центах (1/100 USD).
pub type Cents = i128;

/// Сторона позиции/ордера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosSide {
    Long,
    Short,
}

/// Стоимость `price · qty` в центах: `price_raw · qty_raw · 100 / 10^(pd+qd)`.
fn value_cents(price_raw: i64, qty_raw: i64, pd: u8, qd: u8) -> Cents {
    price_raw as i128 * qty_raw as i128 * 100 / 10i128.pow(pd as u32 + qd as u32)
}

/// Открытая позиция.
#[derive(Debug, Clone)]
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
}

impl Position {
    /// Нереализованный P&L при марк-цене.
    pub fn unrealized(&self, mark_raw: i64) -> Cents {
        let diff = match self.side {
            PosSide::Long => mark_raw - self.entry,
            PosSide::Short => self.entry - mark_raw,
        };
        value_cents(diff, self.qty, self.pd, self.qd)
    }
}

/// Отложенный (лимитный) ордер: срабатывает, когда цена достигает `price`.
#[derive(Debug, Clone)]
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

/// Закрытая сделка (история).
#[derive(Debug, Clone)]
pub struct ClosedDeal {
    pub instrument: u32,
    pub side: PosSide,
    pub qty: i64,
    pub entry: i64,
    pub exit: i64,
    pub pnl: Cents,
    pub pd: u8,
    pub qd: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerError {
    InsufficientMargin,
    UnknownPosition,
    UnknownPending,
    /// Вывод больше, чем свободно (за вычетом занятого под маржу).
    InsufficientFunds,
}

#[derive(Debug, Default, Clone)]
struct Account {
    balance: Cents,
    next_id: u64,
    positions: HashMap<u64, Position>,
    pendings: HashMap<u64, PendingOrder>,
    closed: Vec<ClosedDeal>,
}

impl Account {
    fn used_margin(&self) -> Cents {
        self.positions.values().map(|p| p.margin).sum()
    }

    #[allow(clippy::too_many_arguments)]
    fn try_open(&mut self, lev: i128, instrument: u32, side: PosSide, qty: i64, entry: i64, pd: u8, qd: u8, sl: Option<i64>, tp: Option<i64>) -> Result<u64, BrokerError> {
        let margin = value_cents(entry, qty, pd, qd) / lev;
        if self.balance - self.used_margin() < margin {
            return Err(BrokerError::InsufficientMargin);
        }
        self.next_id += 1;
        let id = self.next_id;
        self.positions.insert(id, Position { id, instrument, side, qty, entry, pd, qd, margin, sl, tp });
        Ok(id)
    }

    fn close_pos(&mut self, id: u64, mark: i64) -> Option<Cents> {
        let pos = self.positions.remove(&id)?;
        let pnl = pos.unrealized(mark);
        self.balance += pnl;
        self.closed.push(ClosedDeal { instrument: pos.instrument, side: pos.side, qty: pos.qty, entry: pos.entry, exit: mark, pnl, pd: pos.pd, qd: pos.qd });
        Some(pnl)
    }

    /// Прогнать триггеры. Возвращает `true`, если состояние счёта изменилось —
    /// по этому признаку gateway понимает, какие счета надо сохранить (ADR-016).
    fn run_triggers(&mut self, lev: i128, marks: &HashMap<u32, i64>) -> bool {
        // Сработавшие лимитные ордера → открыть позицию.
        let mut fire: Vec<PendingOrder> = Vec::new();
        self.pendings.retain(|_, p| match marks.get(&p.instrument) {
            Some(&m) => {
                let hit = match p.side {
                    PosSide::Long => m <= p.price,  // buy limit — ниже рынка
                    PosSide::Short => m >= p.price, // sell limit — выше рынка
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
            let _ = self.try_open(lev, p.instrument, p.side, p.qty, p.price, p.pd, p.qd, p.sl, p.tp);
        }

        // SL/TP по открытым позициям.
        let mut to_close: Vec<(u64, i64)> = Vec::new();
        for pos in self.positions.values() {
            if let Some(&m) = marks.get(&pos.instrument) {
                let sl_hit = pos.sl.is_some_and(|sl| match pos.side {
                    PosSide::Long => m <= sl,
                    PosSide::Short => m >= sl,
                });
                let tp_hit = pos.tp.is_some_and(|tp| match pos.side {
                    PosSide::Long => m >= tp,
                    PosSide::Short => m <= tp,
                });
                if sl_hit {
                    to_close.push((pos.id, pos.sl.unwrap()));
                } else if tp_hit {
                    to_close.push((pos.id, pos.tp.unwrap()));
                }
            }
        }
        let closed_any = !to_close.is_empty();
        for (id, mark) in to_close {
            self.close_pos(id, mark);
        }
        fired_any || closed_any
    }
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
        self.accounts.entry(user).or_insert_with(|| Account { balance: start, ..Default::default() })
    }

    // ---- Чтение -----------------------------------------------------------

    pub fn balance(&self, user: UserId) -> Cents {
        self.accounts.get(&user).map(|a| a.balance).unwrap_or(self.start_balance)
    }
    pub fn used_margin(&self, user: UserId) -> Cents {
        self.accounts.get(&user).map(|a| a.used_margin()).unwrap_or(0)
    }
    pub fn free_margin(&self, user: UserId) -> Cents {
        self.balance(user) - self.used_margin(user)
    }
    pub fn positions(&self, user: UserId) -> Vec<Position> {
        self.accounts.get(&user).map(|a| a.positions.values().cloned().collect()).unwrap_or_default()
    }
    pub fn pendings(&self, user: UserId) -> Vec<PendingOrder> {
        self.accounts.get(&user).map(|a| a.pendings.values().cloned().collect()).unwrap_or_default()
    }
    pub fn closed_deals(&self, user: UserId) -> Vec<ClosedDeal> {
        self.accounts.get(&user).map(|a| a.closed.clone()).unwrap_or_default()
    }
    pub fn open_pnl(&self, user: UserId, marks: &HashMap<u32, i64>) -> Cents {
        self.accounts
            .get(&user)
            .map(|a| a.positions.values().map(|p| p.unrealized(*marks.get(&p.instrument).unwrap_or(&p.entry))).sum())
            .unwrap_or(0)
    }
    pub fn equity(&self, user: UserId, marks: &HashMap<u32, i64>) -> Cents {
        self.balance(user) + self.open_pnl(user, marks)
    }

    // ---- Операции ---------------------------------------------------------

    /// Открыть позицию по рынку (с опциональными SL/TP).
    #[allow(clippy::too_many_arguments)]
    pub fn open(&mut self, user: UserId, instrument: u32, side: PosSide, qty: i64, entry: i64, pd: u8, qd: u8, sl: Option<i64>, tp: Option<i64>) -> Result<u64, BrokerError> {
        let lev = self.leverage;
        self.acct(user).try_open(lev, instrument, side, qty, entry, pd, qd, sl, tp)
    }

    /// Закрыть позицию по марк-цене. Возвращает реализованный P&L.
    pub fn close(&mut self, user: UserId, id: u64, mark: i64) -> Result<Cents, BrokerError> {
        self.acct(user).close_pos(id, mark).ok_or(BrokerError::UnknownPosition)
    }

    /// Пополнить счёт (демо: бумажные деньги). Возвращает новый баланс.
    pub fn deposit(&mut self, user: UserId, amount: Cents) -> Cents {
        let a = self.acct(user);
        a.balance += amount;
        a.balance
    }

    /// Вывести со счёта. Нельзя вывести больше, чем свободно (баланс минус занятое
    /// под маржу открытых позиций). Возвращает новый баланс.
    pub fn withdraw(&mut self, user: UserId, amount: Cents) -> Result<Cents, BrokerError> {
        if amount > self.free_margin(user) {
            return Err(BrokerError::InsufficientFunds);
        }
        let a = self.acct(user);
        a.balance -= amount;
        Ok(a.balance)
    }

    /// Разместить отложенный (лимитный) ордер. Возвращает его id.
    #[allow(clippy::too_many_arguments)]
    pub fn place_pending(&mut self, user: UserId, instrument: u32, side: PosSide, qty: i64, price: i64, pd: u8, qd: u8, sl: Option<i64>, tp: Option<i64>) -> u64 {
        let a = self.acct(user);
        a.next_id += 1;
        let id = a.next_id;
        a.pendings.insert(id, PendingOrder { id, instrument, side, qty, price, pd, qd, sl, tp });
        id
    }

    pub fn cancel_pending(&mut self, user: UserId, id: u64) -> Result<(), BrokerError> {
        if self.acct(user).pendings.remove(&id).is_some() {
            Ok(())
        } else {
            Err(BrokerError::UnknownPending)
        }
    }

    /// Прогнать триггеры лимитных ордеров и SL/TP по всем счетам при текущих марк-ценах.
    ///
    /// Возвращает **id счетов, состояние которых изменилось**. Обычный случай — цена
    /// движется, но ничего не сработало → пустой вектор, сохранять нечего. Без этого
    /// пришлось бы писать в БД все счета на каждом тике монитора (ADR-016).
    pub fn check(&mut self, marks: &HashMap<u32, i64>) -> Vec<UserId> {
        let lev = self.leverage;
        let mut changed = Vec::new();
        for (user, a) in self.accounts.iter_mut() {
            if a.run_triggers(lev, marks) {
                changed.push(*user);
            }
        }
        changed
    }

    // ---- Персистентность (ADR-016) ----------------------------------------
    //
    // Брокер остаётся in-memory и синхронным: он не знает ни про БД, ни про I/O
    // (принцип №6). Наружу отдаётся слепок счёта, а сохраняет и загружает его
    // адаптер в gateway через крейт `persistence`.

    /// Слепок счёта для сохранения. Позиции и ордера переписываются целиком —
    /// их единицы-десятки; история закрытых сделок только дописывается.
    pub fn snapshot(&self, user: UserId) -> AccountSnapshot {
        match self.accounts.get(&user) {
            Some(a) => AccountSnapshot {
                balance: a.balance,
                next_id: a.next_id,
                positions: a.positions.values().cloned().collect(),
                pendings: a.pendings.values().cloned().collect(),
                closed: a.closed.clone(),
            },
            None => AccountSnapshot { balance: self.start_balance, next_id: 0, positions: vec![], pendings: vec![], closed: vec![] },
        }
    }

    /// Восстановить счёт из хранилища на старте. Полностью заменяет состояние счёта.
    pub fn restore(&mut self, user: UserId, snap: AccountSnapshot) {
        let a = Account {
            balance: snap.balance,
            next_id: snap.next_id,
            positions: snap.positions.into_iter().map(|p| (p.id, p)).collect(),
            pendings: snap.pendings.into_iter().map(|p| (p.id, p)).collect(),
            closed: snap.closed,
        };
        self.accounts.insert(user, a);
    }

    /// Все счета, о которых знает брокер (для сохранения/обхода).
    pub fn users(&self) -> Vec<UserId> {
        self.accounts.keys().copied().collect()
    }
}

/// Слепок состояния одного счёта — единица обмена с хранилищем (ADR-016).
#[derive(Debug, Clone, Default)]
pub struct AccountSnapshot {
    pub balance: Cents,
    pub next_id: u64,
    pub positions: Vec<Position>,
    pub pendings: Vec<PendingOrder>,
    pub closed: Vec<ClosedDeal>,
}
