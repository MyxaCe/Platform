//! # domain
//!
//! Общий доменный словарь биржи — типы, разделяемые сервисами (matching-ядро `core`,
//! `ledger` и будущие). Здесь только типы и реестр инструментов, без бизнес-логики
//! конкретного сервиса (ADR-007).

pub mod account;
pub mod instrument;
pub mod money;
pub mod order;
