//! Целочисленное представление денег/цен/объёмов (ADR-002).
//!
//! Никакого float. Все величины — целые в *минимальных единицах* инструмента.
//! Newtypes не дают случайно смешать цену с объёмом на уровне типов.

/// Цена в минимальных единицах (реальная цена = `Price / 10^price_decimals`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Price(pub i64);

/// Объём в минимальных единицах (реальный объём = `Qty / 10^qty_decimals`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Qty(pub i64);

impl Qty {
    pub const ZERO: Qty = Qty(0);

    /// Объём равен нулю (заявка исчерпана).
    #[inline]
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Объём строго положителен (валидная заявка).
    #[inline]
    pub fn is_positive(self) -> bool {
        self.0 > 0
    }
}

impl std::ops::Sub for Qty {
    type Output = Qty;
    /// Разность объёмов. `debug_assert` ловит уход в минус — это баг-инвариант матчинга.
    #[inline]
    fn sub(self, rhs: Qty) -> Qty {
        debug_assert!(self.0 >= rhs.0, "Qty ушёл в минус: {} - {}", self.0, rhs.0);
        Qty(self.0 - rhs.0)
    }
}

impl std::ops::Add for Qty {
    type Output = Qty;
    #[inline]
    fn add(self, rhs: Qty) -> Qty {
        Qty(self.0 + rhs.0)
    }
}

/// Денежная сумма для балансов Ledger'а — целочисленный newtype поверх `i128`
/// (шире `i64`, т.к. нотионал = цена × объём может быть большим). См. ADR-006.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Amount(pub i128);

impl Amount {
    pub const ZERO: Amount = Amount(0);

    #[inline]
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
    #[inline]
    pub fn is_positive(self) -> bool {
        self.0 > 0
    }
    #[inline]
    pub fn is_negative(self) -> bool {
        self.0 < 0
    }
}

impl std::ops::Add for Amount {
    type Output = Amount;
    #[inline]
    fn add(self, rhs: Amount) -> Amount {
        Amount(self.0 + rhs.0)
    }
}

impl std::ops::Sub for Amount {
    type Output = Amount;
    #[inline]
    fn sub(self, rhs: Amount) -> Amount {
        Amount(self.0 - rhs.0)
    }
}

/// Нотионал сделки (цена × объём) как [`Amount`] — считается в `i128` без переполнения.
#[inline]
pub fn notional(price: Price, qty: Qty) -> Amount {
    Amount(price.0 as i128 * qty.0 as i128)
}
