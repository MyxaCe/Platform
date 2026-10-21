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
//! [`MemoryStore`] (всё в памяти процесса) и `PgStore` (PostgreSQL). Журнал событий
//! счёта (TRM-13) живёт здесь же, рядом со слепком, и пишется с ним одной транзакцией.
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
//!
//! ## Слепок и журнал пишутся вместе (TRM-13)
//!
//! `save_account` принимает и слепок, и новые записи журнала, и кладёт их **одной
//! транзакцией**. Иначе мы сами бы производили то расхождение, которое журнал заведён
//! обнаруживать: упавшая между двумя записями транзакция оставила бы остаток без
//! проводки или проводку без остатка.

pub mod money;
mod pg;

pub use pg::PgStore;

use async_trait::async_trait;

use broker::journal::AccountEvent;
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
    /// Журналы счетов (TRM-13), в порядке записи. Счёт без журнала здесь просто
    /// отсутствует — и это **не** то же самое, что недоступное хранилище: туда мы
    /// вообще не доходим, чтение отдаёт [`StoreError`].
    pub journals: Vec<(UserId, Vec<AccountEvent>)>,
}

/// Контракт хранилища состояния.
///
/// `double_must_use` разрешён точечно: атрибут ставит не наш код, а раскрытие
/// `#[async_trait]` — оно помечает `#[must_use]` метод, уже возвращающий
/// `Pin<Box<dyn Future>>`, который и так `#[must_use]`. Лint появился в clippy
/// 1.99 и красит чужую генерацию, а не наше решение; переписать её мы не можем,
/// а глушить предупреждения целиком в CI (`-D warnings` отключить) значило бы
/// потерять все остальные.
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait Persistence: Send + Sync {
    /// Подготовить хранилище (идемпотентный DDL). Вызывается один раз на старте.
    async fn init(&self) -> Result<(), StoreError>;

    /// Поднять всё состояние. Вызывается один раз на старте, до приёма запросов.
    async fn load_all(&self) -> Result<LoadedState, StoreError>;

    /// Сохранить слепок счёта и новые записи его журнала **одной транзакцией**:
    /// позиции и отложенные ордера переписываются целиком, история закрытых сделок и
    /// журнал — только дописываются.
    ///
    /// `events` уже запечатаны вызывающим (номер, время, источник, отпечаток): здесь
    /// их содержимое не меняется и не досочиняется.
    async fn save_account(
        &self,
        user: UserId,
        snap: &AccountSnapshot,
        events: &[AccountEvent],
    ) -> Result<(), StoreError>;

    /// Найти или создать внутренний `UserId` по внешней личности `(tenant, sub)` из
    /// провалидированного SSO-токена (ADR-023, фаза Т2). Маппинг стабилен между
    /// рестартами — иначе после перезапуска пользователь получил бы новый пустой счёт.
    async fn resolve_identity(&self, _tenant: &str, _sub: &str) -> Result<UserId, StoreError> {
        Err(StoreError("identity mapping requires a database".into()))
    }

    /// Переживает ли состояние перезапуск процесса.
    ///
    /// Нужен не коду, а человеку: в памяти счёт после рестарта выглядит как настоящий
    /// счёт со стартовым балансом, и отличить «состояние потеряно» от «я ничего не
    /// наторговал» изнутри интерфейса нельзя (BUG-011). Отдаётся в `/health`, чтобы
    /// режим был проверяем машиной, а не только глазами. Дефолт — `false`: хранилище,
    /// которое не заявило durability явно, считаем недолговечным.
    fn durable(&self) -> bool {
        false
    }

    /// Человекочитаемое имя режима хранилища (для `/health` и логов).
    fn kind(&self) -> &'static str {
        "memory"
    }

    // Денежные инварианты проверяются НЕ здесь: они считаются над слепком счёта
    // (`broker::check_snapshot`) и потому работают одинаково с БД и без неё.
    // Прежняя проверка жила в хранилище, у `NoopStore` была пустой, а на
    // `NoopStore` идут все тесты — то есть не выполнялась ни разу (TRM-06).
}

/// Хранилище в памяти процесса: режим без `DATABASE_URL` (локальный запуск и тесты).
///
/// **Пришло на смену `NoopStore`, и это не косметика.** `NoopStore` ничего не делал, а
/// все тесты шли на нём — то есть путь записи не выполнялся ни разу, и ровно так у нас
/// однажды «работал» пустой инвариант (TRM-06). Здесь путь записи настоящий: события
/// складываются в ленту, номера проверяются, подъём возвращает то же, что положили.
/// Durability от этого не появляется — состояние живёт, пока жив процесс, — но **код**,
/// который с БД отвечает за журнал, в тестах выполняется, а не обходится стороной.
#[derive(Default)]
pub struct MemoryStore {
    accounts: std::sync::Mutex<std::collections::HashMap<UserId, AccountSnapshot>>,
    journals: std::sync::Mutex<std::collections::HashMap<UserId, Vec<AccountEvent>>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Persistence for MemoryStore {
    async fn init(&self) -> Result<(), StoreError> {
        Ok(())
    }

    async fn load_all(&self) -> Result<LoadedState, StoreError> {
        let accounts = self
            .accounts
            .lock()
            .map_err(|_| StoreError("память счетов испорчена".into()))?;
        let journals = self
            .journals
            .lock()
            .map_err(|_| StoreError("память журналов испорчена".into()))?;
        let mut acc: Vec<(UserId, AccountSnapshot)> =
            accounts.iter().map(|(u, s)| (*u, s.clone())).collect();
        acc.sort_by_key(|(u, _)| u.0);
        let mut jr: Vec<(UserId, Vec<AccountEvent>)> =
            journals.iter().map(|(u, e)| (*u, e.clone())).collect();
        jr.sort_by_key(|(u, _)| u.0);
        Ok(LoadedState {
            accounts: acc,
            journals: jr,
        })
    }

    async fn save_account(
        &self,
        user: UserId,
        snap: &AccountSnapshot,
        events: &[AccountEvent],
    ) -> Result<(), StoreError> {
        let mut accounts = self
            .accounts
            .lock()
            .map_err(|_| StoreError("память счетов испорчена".into()))?;
        let mut journals = self
            .journals
            .lock()
            .map_err(|_| StoreError("память журналов испорчена".into()))?;
        let lane = journals.entry(user).or_default();
        // Только добавление, и только в хвост. Повтор номера — отказ, как и уникальный
        // ключ в БД: журнал, в который можно записать «поверх», журналом не является.
        for e in events {
            let expected = lane.last().map(|x: &AccountEvent| x.seq()).unwrap_or(0) + 1;
            if e.seq() != expected {
                return Err(StoreError(format!(
                    "журнал счёта {}: ожидалась запись №{expected}, пришла №{}",
                    user.0,
                    e.seq()
                )));
            }
            lane.push(e.clone());
        }
        accounts.insert(user, snap.clone());
        Ok(())
    }
}
