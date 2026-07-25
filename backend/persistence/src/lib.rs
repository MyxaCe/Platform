//! # persistence
//!
//! Durable-хранилище изменяемого состояния терминала (ADR-016): счета брокера
//! (баланс, позиции, отложенные ордера, история сделок).
//!
//! ## Роли
//!
//! Память — рабочее состояние, хранилище — источник восстановления. [`broker::Broker`]
//! остаётся in-memory и синхронным, без I/O (принцип №6): иначе тик монитора каждые
//! 500 мс превратился бы в чтение всех позиций всех пользователей из БД.
//!
//! - **на старте** gateway читает хранилище и наполняет `Broker` и реестр токенов;
//! - **при каждой мутации** адаптер в gateway записывает слепок затронутого счёта.
//!
//! ## Почему трейт
//!
//! Реализация меняется, не задевая логику брокера и обработчики gateway: сейчас
//! [`NoopStore`] (всё в памяти, как раньше) и `PgStore` (PostgreSQL); в Фазе 3
//! состояние переедет на журнал событий — граница останется здесь же.
//!
//! ## Порядок записи (важно)
//!
//! `fsync`/сеть под локом брокера недопустимы — это заблокировало бы торговлю всех
//! пользователей. Дисциплина вызывающего:
//!
//! 1. под локом мутировать брокер и снять слепок затронутого счёта;
//! 2. **отпустить лок**;
//! 3. `save_account(...)` — запись одной транзакцией;
//! 4. не прошло — вернуть счёт к предыдущему слепку (под локом) и ответить `503`.

pub mod money;
mod pg;

pub use pg::PgStore;

use async_trait::async_trait;

use broker::AccountSnapshot;
use domain::account::UserId;

/// Ошибка хранилища. Наверх поднимается как `503`: данные пользователя не потеряны,
/// но и не сохранены — операцию следует считать несостоявшейся.
#[derive(Debug, Clone)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "store: {}", self.0)
    }
}
impl std::error::Error for StoreError {}

/// Всё состояние, поднимаемое из хранилища на старте.
#[derive(Debug, Default)]
pub struct LoadedState {
    /// Счета брокера.
    pub accounts: Vec<(UserId, AccountSnapshot)>,
}

/// Контракт хранилища состояния.
#[async_trait]
pub trait Persistence: Send + Sync {
    /// Подготовить хранилище (идемпотентный DDL). Вызывается один раз на старте.
    async fn init(&self) -> Result<(), StoreError>;

    /// Поднять всё состояние. Вызывается один раз на старте, до приёма запросов.
    async fn load_all(&self) -> Result<LoadedState, StoreError>;

    /// Сохранить слепок счёта одной транзакцией: позиции и отложенные ордера
    /// переписываются целиком, история закрытых сделок дописывается.
    async fn save_account(&self, user: UserId, snap: &AccountSnapshot) -> Result<(), StoreError>;

    /// Проверка денежных инвариантов (красная линия №5). Возвращает список нарушений;
    /// пустой вектор — всё сходится. Вызывается на старте после загрузки.
    async fn check_invariants(&self) -> Result<Vec<String>, StoreError> {
        Ok(Vec::new())
    }
}

/// Заглушка: ничего не хранит. Поведение платформы ровно как до ADR-016 —
/// перезапуск обнуляет состояние. Используется, когда `DATABASE_URL` не задан
/// (локальный запуск и существующие тесты не требуют живой БД).
pub struct NoopStore;

#[async_trait]
impl Persistence for NoopStore {
    async fn init(&self) -> Result<(), StoreError> {
        Ok(())
    }
    async fn load_all(&self) -> Result<LoadedState, StoreError> {
        Ok(LoadedState::default())
    }
    async fn save_account(&self, _user: UserId, _snap: &AccountSnapshot) -> Result<(), StoreError> {
        Ok(())
    }
}
