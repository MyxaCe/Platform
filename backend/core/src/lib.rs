//! # exchange_core
//!
//! Детерминированное matching-ядро биржи (Фаза 1): order book + matching engine.
//! Доменные типы берутся из крейта [`domain`] (ADR-007).
//!
//! Архитектура — событийная (см. ADR-003): ядро принимает [`Command`], возвращает
//! неизменяемый список [`Event`]. Никакого I/O, времени и случайности внутри — всё
//! недетерминированное инжектится снаружи (в командах). Один и тот же вход всегда даёт
//! один и тот же выход. Матчинг идёт по инструментам (ADR-005).
//!
//! Точка входа — [`MatchingEngine::apply`].

pub mod book;
pub mod command;
pub mod engine;
pub mod event;

// Публичный фасад ядра — то, чем пользуются адаптеры (шлюз ордеров, тесты и т.д.).
pub use book::{DepthSnapshot, Level, OrderBook, RestingOrder};
pub use command::Command;
pub use engine::MatchingEngine;
pub use event::{Event, RejectReason, TradeId};

// Re-export доменных типов для удобства потребителей ядра.
pub use domain::instrument::{AssetId, Instrument, InstrumentId, InstrumentRegistry};
pub use domain::money::{Amount, Price, Qty};
pub use domain::order::{OrderId, OrderType, Side, TimeInForce};
