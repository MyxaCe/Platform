//! Команды — единственный вход ядра.
//!
//! Команда описывает *намерение* внешнего мира. Всё недетерминированное (уникальный id,
//! при необходимости — время) уже проставлено вызывающей стороной до попадания в ядро.
//! Каждая команда адресована конкретному инструменту (ADR-005).

use domain::instrument::InstrumentId;
use domain::money::Qty;
use domain::order::{OrderId, OrderType, Side, TimeInForce};

/// Команда к matching engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Разместить заявку в книге инструмента `instrument`.
    PlaceOrder {
        instrument: InstrumentId,
        id: OrderId,
        side: Side,
        order_type: OrderType,
        qty: Qty,
        tif: TimeInForce,
    },
    /// Снять ранее размещённую заявку, стоящую в книге инструмента `instrument`.
    CancelOrder {
        instrument: InstrumentId,
        id: OrderId,
    },
}
