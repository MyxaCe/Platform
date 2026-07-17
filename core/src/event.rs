//! События — единственный выход ядра.
//!
//! События неизменяемы и упорядочены; это будущий журнал-истина (ADR-003). Все проекции
//! (балансы, стакан, свечи, история) выводятся из потока событий.

use crate::domain::money::{Price, Qty};
use crate::domain::order::{OrderId, Side};

/// Id сделки. Монотонный счётчик из состояния движка — детерминирован (не из времени).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TradeId(pub u64);

/// Причина отклонения команды.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// Объём заявки не положителен.
    NonPositiveQty,
    /// Попытка снять заявку, которой нет в стакане.
    UnknownOrder,
}

/// Событие, порождённое ядром.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Заявка принята к обработке (прошла валидацию).
    OrderAccepted { id: OrderId },

    /// Совершена сделка. Цена — цена maker'а (стоявшей в стакане заявки).
    Trade {
        id: TradeId,
        price: Price,
        qty: Qty,
        /// Инициатор (входящая заявка).
        taker: OrderId,
        /// Контрагент (стоял в стакане).
        maker: OrderId,
        /// Сторона taker'а.
        taker_side: Side,
    },

    /// Заявка (или её остаток) встала в стакан как maker.
    OrderResting { id: OrderId, price: Price, qty: Qty },

    /// Заявка полностью исполнена.
    OrderFilled { id: OrderId },

    /// Остаток заявки отменён без постановки в стакан (IOC либо рыночная без ликвидности).
    OrderCanceledRemainder { id: OrderId, qty: Qty },

    /// Заявка снята по команде пользователя.
    OrderCanceled { id: OrderId },

    /// Команда отклонена.
    OrderRejected { id: OrderId, reason: RejectReason },
}
