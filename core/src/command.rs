//! Команды — единственный вход ядра.
//!
//! Команда описывает *намерение* внешнего мира. Всё недетерминированное (уникальный id,
//! при необходимости — время) уже проставлено вызывающей стороной до попадания в ядро.

use crate::domain::money::Qty;
use crate::domain::order::{OrderId, OrderType, Side, TimeInForce};

/// Команда к matching engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Разместить заявку.
    PlaceOrder {
        id: OrderId,
        side: Side,
        order_type: OrderType,
        qty: Qty,
        tif: TimeInForce,
    },
    /// Снять ранее размещённую заявку, стоящую в стакане.
    CancelOrder { id: OrderId },
}
