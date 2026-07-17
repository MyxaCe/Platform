//! # exchange_core
//!
//! Детерминированное ядро биржи (Фаза 1): доменные типы, order book, matching engine.
//!
//! Архитектура — событийная (см. ADR-003): ядро принимает [`Command`], возвращает
//! неизменяемый список [`Event`]. Никакого I/O, времени и случайности внутри — всё
//! недетерминированное инжектится снаружи (в командах). Один и тот же вход всегда даёт
//! один и тот же выход. Матчинг идёт по инструментам (ADR-005).
//!
//! Точка входа — [`MatchingEngine::apply`].

pub mod book;
pub mod command;
pub mod domain;
pub mod engine;
pub mod event;

// Публичный фасад ядра — то, чем пользуются адаптеры (шлюз ордеров, тесты и т.д.).
pub use book::{DepthSnapshot, Level, OrderBook, RestingOrder};
pub use command::Command;
pub use domain::instrument::{AssetId, Instrument, InstrumentId, InstrumentRegistry};
pub use domain::money::{Price, Qty};
pub use domain::order::{OrderId, OrderType, Side, TimeInForce};
pub use engine::MatchingEngine;
pub use event::{Event, RejectReason, TradeId};
