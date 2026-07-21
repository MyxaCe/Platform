//! # ledger
//!
//! Счета и балансы биржи с двойной записью (ADR-006). Счёт = `(UserId, AssetId)`, баланс —
//! `available` (свободно) + `held` (зарезервировано). Все операции балансовы: сумма изменений
//! по активу равна нулю, кроме явных депозита/вывода. Инвариант — `total_supply(asset)`
//! постоянна между депозитами/выводами.
//!
//! Ledger не зависит от matching-ядра; он получает факты сделок снаружи (оркестратор Фазы 2b).

use std::collections::HashMap;

use domain::account::UserId;
use domain::instrument::AssetId;
use domain::money::{Amount, Price, Qty};

/// Баланс одного счёта.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Balance {
    /// Свободные средства.
    pub available: Amount,
    /// Зарезервированные под активные заявки средства.
    pub held: Amount,
}

impl Balance {
    /// Полный баланс (`available + held`).
    #[inline]
    pub fn total(self) -> Amount {
        self.available + self.held
    }
}

/// Ошибка балансовой операции.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerError {
    /// Недостаточно свободных средств для резерва/вывода.
    InsufficientAvailable,
    /// Недостаточно зарезервированных средств для освобождения/расчёта (баг-инвариант вызывающего).
    InsufficientHeld,
}

/// Реестр балансов пользователей по активам.
#[derive(Debug, Default)]
pub struct Ledger {
    balances: HashMap<(UserId, AssetId), Balance>,
}

impl Ledger {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- Чтение -----------------------------------------------------------

    pub fn balance(&self, user: UserId, asset: AssetId) -> Balance {
        self.balances.get(&(user, asset)).copied().unwrap_or_default()
    }

    pub fn available(&self, user: UserId, asset: AssetId) -> Amount {
        self.balance(user, asset).available
    }

    pub fn held(&self, user: UserId, asset: AssetId) -> Amount {
        self.balance(user, asset).held
    }

    /// Суммарный объём актива по всем счетам (`available + held`). Для проверки инварианта сходимости.
    pub fn total_supply(&self, asset: AssetId) -> Amount {
        self.balances
            .iter()
            .filter(|((_, a), _)| *a == asset)
            .fold(Amount::ZERO, |acc, (_, b)| acc + b.total())
    }

    // ---- Изменение --------------------------------------------------------

    fn entry(&mut self, user: UserId, asset: AssetId) -> &mut Balance {
        self.balances.entry((user, asset)).or_default()
    }

    /// Внешнее пополнение: `available += amount`. Меняет `total_supply`.
    pub fn deposit(&mut self, user: UserId, asset: AssetId, amount: Amount) {
        debug_assert!(amount.is_positive(), "депозит должен быть положительным");
        let b = self.entry(user, asset);
        b.available = b.available + amount;
    }

    /// Внешний вывод: `available -= amount`. Меняет `total_supply`.
    pub fn withdraw(&mut self, user: UserId, asset: AssetId, amount: Amount) -> Result<(), LedgerError> {
        let b = self.entry(user, asset);
        if b.available < amount {
            return Err(LedgerError::InsufficientAvailable);
        }
        b.available = b.available - amount;
        Ok(())
    }

    /// Резерв под заявку: `available → held`.
    pub fn reserve(&mut self, user: UserId, asset: AssetId, amount: Amount) -> Result<(), LedgerError> {
        let b = self.entry(user, asset);
        if b.available < amount {
            return Err(LedgerError::InsufficientAvailable);
        }
        b.available = b.available - amount;
        b.held = b.held + amount;
        Ok(())
    }

    /// Возврат резерва (отмена/остаток): `held → available`.
    pub fn release(&mut self, user: UserId, asset: AssetId, amount: Amount) -> Result<(), LedgerError> {
        let b = self.entry(user, asset);
        if b.held < amount {
            return Err(LedgerError::InsufficientHeld);
        }
        b.held = b.held - amount;
        b.available = b.available + amount;
        Ok(())
    }

    /// Расчёт сделки (ADR-006). Атомарно и балансово переводит средства между покупателем и
    /// продавцом, списывая из ранее зарезервированного (`held`).
    ///
    /// - `base`/`quote` — активы инструмента;
    /// - `price` — цена исполнения (цена maker'а), `qty` — объём;
    /// - `buyer_reserve_price` — цена, по которой покупатель резервировал `quote` (его лимит);
    ///   разница `(reserve − fill) × qty` возвращается покупателю (price improvement).
    ///
    /// Недостаток `held` означает баг вызывающего (резерв не был сделан) → `InsufficientHeld`.
    #[allow(clippy::too_many_arguments)]
    pub fn settle_fill(
        &mut self,
        base: AssetId,
        quote: AssetId,
        buyer: UserId,
        seller: UserId,
        price: Price,
        qty: Qty,
        buyer_reserve_price: Price,
    ) -> Result<(), LedgerError> {
        let q = qty.0 as i128;
        let fill_notional = Amount(price.0 as i128 * q);
        let reserved_notional = Amount(buyer_reserve_price.0 as i128 * q);
        let refund = reserved_notional - fill_notional; // ≥ 0, если reserve ≥ fill
        let base_qty = Amount(q);

        // Достаточность резерва (иначе — баг вызывающего).
        if self.held(buyer, quote) < reserved_notional {
            return Err(LedgerError::InsufficientHeld);
        }
        if self.held(seller, base) < base_qty {
            return Err(LedgerError::InsufficientHeld);
        }

        // Покупатель: снять резерв quote, вернуть разницу, получить base.
        {
            let b = self.entry(buyer, quote);
            b.held = b.held - reserved_notional;
            b.available = b.available + refund;
        }
        {
            let b = self.entry(buyer, base);
            b.available = b.available + base_qty;
        }
        // Продавец: снять резерв base, получить quote.
        {
            let s = self.entry(seller, base);
            s.held = s.held - base_qty;
        }
        {
            let s = self.entry(seller, quote);
            s.available = s.available + fill_notional;
        }

        Ok(())
    }
}
