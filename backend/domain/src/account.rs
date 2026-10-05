//! Идентификатор пользователя — владельца торгового счёта у брокера (крейт `broker`).

/// Глобально уникальный id пользователя.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UserId(pub u64);
