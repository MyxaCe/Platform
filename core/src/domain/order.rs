//! Заявка (order) и её атрибуты.

use crate::domain::money::Price;

/// Сторона заявки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    /// Противоположная сторона (та, против которой матчимся).
    #[inline]
    pub fn opposite(self) -> Side {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

/// Глобально уникальный id заявки. Назначается снаружи ядра (шлюзом ордеров),
/// потому что уникальность — это ответственность границы, а не детерминированного ядра.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OrderId(pub u64);

/// Тип заявки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderType {
    /// Лимитная: не исполняется хуже `price`; неисполненный остаток может встать в стакан.
    Limit { price: Price },
    /// Рыночная: исполняется по любым доступным ценам; остаток в стакан НЕ встаёт.
    Market,
}

/// Время жизни заявки (актуально для лимитных).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeInForce {
    /// Good-Till-Cancel: остаток стоит в стакане, пока его не отменят.
    Gtc,
    /// Immediate-Or-Cancel: исполнить что можно сейчас, остаток — отменить.
    Ioc,
}
