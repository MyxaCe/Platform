//! Идентификатор пользователя — владельца счетов в Ledger'е.

/// Глобально уникальный id пользователя.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UserId(pub u64);
