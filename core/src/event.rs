//! События — единственный выход ядра.
//!
//! События неизменяемы и упорядочены; это будущий журнал-истина (ADR-003). Все проекции
//! (балансы, стакан, свечи, история) выводятся из потока событий. Журнал глобальный и
//! чересполосный, поэтому **каждое событие самодостаточно** и несёт свой `instrument` (ADR-005).

use crate::domain::instrument::InstrumentId;
use crate::domain::money::{Price, Qty};
use crate::domain::order::{OrderId, Side};

/// Id сделки. Монотонный счётчик из состояния движка — детерминирован (не из времени).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TradeId(pub u64);

/// Причина отклонения команды.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// Инструмент не зарегистрирован.
    UnknownInstrument,
    /// Объём заявки не положителен.
    NonPositiveQty,
    /// Объём не кратен шагу объёма (`lot_size`).
    QtyNotOnLot,
    /// Объём меньше минимального (`min_qty`).
    BelowMinQty,
    /// Цена лимитной заявки не положительна.
    NonPositivePrice,
    /// Цена не кратна шагу цены (`tick_size`).
    PriceNotOnTick,
    /// Попытка снять заявку, которой нет в книге.
    UnknownOrder,
}

/// Событие, порождённое ядром.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Заявка принята к обработке (прошла валидацию).
    OrderAccepted { instrument: InstrumentId, id: OrderId },

    /// Совершена сделка. Цена — цена maker'а (стоявшей в стакане заявки).
    Trade {
        instrument: InstrumentId,
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
    OrderResting { instrument: InstrumentId, id: OrderId, price: Price, qty: Qty },

    /// Заявка полностью исполнена.
    OrderFilled { instrument: InstrumentId, id: OrderId },

    /// Остаток заявки отменён без постановки в стакан (IOC либо рыночная без ликвидности).
    OrderCanceledRemainder { instrument: InstrumentId, id: OrderId, qty: Qty },

    /// Заявка снята по команде пользователя.
    OrderCanceled { instrument: InstrumentId, id: OrderId },

    /// Команда отклонена.
    OrderRejected { instrument: InstrumentId, id: OrderId, reason: RejectReason },
}
