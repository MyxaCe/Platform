#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Price(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Qty(pub i64);

impl Qty {
    pub const ZERO: Qty = Qty(0);

    #[inline]
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub fn is_positive(self) -> bool {
        self.0 > 0
    }
}

impl std::ops::Sub for Qty {
    type Output = Qty;
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

#[inline]
pub fn notional(price: Price, qty: Qty) -> Amount {
    Amount(price.0 as i128 * qty.0 as i128)
}
