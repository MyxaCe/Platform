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
//!
//! ## Исходящая очередь шины — там же и той же транзакцией (TRM-40)
//!
//! Событие о сделке, потерянное на перезапуске, — это деньги. Поэтому исходящая
//! очередь **не массив в памяти**, а таблица рядом со слепком и журналом, и
//! наполняется она тем же `save_account`, в той же транзакции.
//!
//! Из этого следует порядок, который легко нарушить и трудно заметить:
//! **публикуем после фиксации, а не до**. Опубликованное до коммита событие
//! переживает откат — наружу уходит сделка, которой не было, и отозвать её
//! нельзя. Пропущенная публикация, наоборот, чинится повтором: строка лежит
//! непомеченной и уедет следующим проходом.

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

/// Запись исходящей очереди шины (TRM-40): готовый к отправке конверт.
///
/// Конверт собран **до** записи и дальше не меняется: релей его только отправляет.
/// Иначе форма события зависела бы от версии кода в момент отправки, а не в момент
/// сделки, и строка, пролежавшая через выкатку, уехала бы в чужом виде.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRecord {
    /// `event_id` конверта. Он же `message_id` сообщения — по нему дедуплицирует
    /// потребитель. **Выводится из факта, а не из случайности** (см. `bus::event_id`):
    /// повторная запись того же факта даёт тот же идентификатор.
    pub event_id: String,
    /// Ключ маршрутизации = имя события (`terminal.trade.executed`).
    pub routing_key: String,
    /// Конверт целиком, JSON.
    pub payload: String,
}

/// Строка исходящей очереди, взятая релеем на отправку.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxClaim {
    pub id: i64,
    pub event_id: String,
    pub routing_key: String,
    pub payload: String,
    /// Сколько раз отправка уже отказывала. Ноль — строку ещё не пробовали.
    pub attempts: i32,
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
    ///
    /// `outgoing` — конверты в шину (TRM-40). Той же транзакцией и по той же
    /// причине: событие, записанное отдельно от денег, рано или поздно разойдётся
    /// с ними — либо наружу уедет сделка, которой не было, либо сделка будет, а
    /// события не будет. Пустой срез — обычный случай (не всякая мутация видна
    /// снаружи), и он ничего не стоит.
    async fn save_account(
        &self,
        user: UserId,
        snap: &AccountSnapshot,
        events: &[AccountEvent],
        outgoing: &[OutboxRecord],
    ) -> Result<(), StoreError>;

    /// Взять пачку неотправленных конвертов, старейшие первыми.
    ///
    /// Взятые строки получают **аренду**: их `next_attempt_at` сдвигается вперёд,
    /// чтобы второй экземпляр не взял то же самое. Аренда — не попытка: `attempts`
    /// растёт только на настоящем отказе, иначе счётчик считал бы заходы релея, а
    /// не отказы шины, и по нему нельзя было бы понять, что происходит.
    /// Умолчания у следующих четырёх методов **отказывают**, а не возвращают
    /// пустоту, и разница здесь принципиальная. Умолчание, которое тихо
    /// УСПЕВАЕТ, — это то самое, на чём мы однажды уже обожглись: у `NoopStore`
    /// проверка инвариантов была пустой, все тесты шли на нём, и набор ни разу
    /// не выполнил проверяемое (TRM-06). Умолчание, которое ОТКАЗЫВАЕТ, так
    /// соврать не может: хранилище, не заявившее исходящую очередь, её не имеет,
    /// и любой, кто на неё обопрётся, узнает об этом сразу.
    ///
    /// Нужны они затем, чтобы двойник в тесте, которому очередь не интересна, не
    /// переписывал пять методов ради одного проверяемого свойства.
    async fn claim_outbox(
        &self,
        _limit: i64,
        _lease_ms: i64,
    ) -> Result<Vec<OutboxClaim>, StoreError> {
        Err(StoreError("хранилище не ведёт исходящую очередь".into()))
    }

    /// Пометить строку отправленной.
    async fn mark_published(&self, _id: i64) -> Result<(), StoreError> {
        Err(StoreError("хранилище не ведёт исходящую очередь".into()))
    }

    /// Записать отказ отправки: номер попытки, когда пробовать снова, и причина.
    ///
    /// Отказ обязан быть виден **в состоянии строки**, а не только в логе процесса:
    /// у соседей ровно этот зазор однажды оставил `attempts = 0` при циклических
    /// падениях — повторы «работали» и не отработали ни разу (КОНТРАКТЫ §4а).
    async fn mark_failed(
        &self,
        _id: i64,
        _attempts: i32,
        _retry_in_ms: i64,
        _error: &str,
    ) -> Result<(), StoreError> {
        Err(StoreError("хранилище не ведёт исходящую очередь".into()))
    }

    /// Сколько конвертов ждёт отправки. Нужен не коду, а человеку: непустая очередь
    /// при молчащей шине — это видимое состояние, а не тишина.
    async fn outbox_backlog(&self) -> Result<i64, StoreError> {
        Err(StoreError("хранилище не ведёт исходящую очередь".into()))
    }

    /// Внешняя личность счёта `(tenant, sub)` — обратная сторона [`Self::resolve_identity`].
    ///
    /// Нужна публикующему: событие адресуется парой «сайт + пользователь сайта», а
    /// внутренний `UserId` за границей терминала не значит ничего. `None` — счёт
    /// заведён не через SSO (standalone), и адресата у события нет вовсе.
    /// Умолчание здесь, наоборот, `Ok(None)` — и это не послабление. `None`
    /// означает «счёт не привязан к сайту», и для хранилища без маппинга
    /// личностей это правда, а не умолчание: `resolve_identity` у него тоже
    /// отказывает, значит и привязки взяться неоткуда.
    async fn identity_of(&self, _user: UserId) -> Result<Option<(String, String)>, StoreError> {
        Ok(None)
    }

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
    /// Исходящая очередь шины. Здесь она **не durable** — как и всё остальное в
    /// этом хранилище, — но путь записи и выборки настоящий, и код релея в тестах
    /// выполняется, а не обходится стороной (тот же урок, что с `NoopStore`).
    outbox: std::sync::Mutex<Vec<MemoryOutboxRow>>,
    next_outbox_id: std::sync::atomic::AtomicI64,
}

#[derive(Debug, Clone)]
struct MemoryOutboxRow {
    id: i64,
    event_id: String,
    routing_key: String,
    payload: String,
    attempts: i32,
    published: bool,
    /// Монотонные миллисекунды «не раньше чем»; сравниваются с [`MemoryStore::now_ms`].
    next_attempt_at: i64,
    last_error: Option<String>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    /// Снимок очереди для проверок: (`event_id`, `attempts`, отправлено, последняя ошибка).
    ///
    /// Нужен тестам, и это сознательно: отказ отправки обязан быть виден в
    /// СОСТОЯНИИ СТРОКИ, а проверить это можно только заглянув в строку.
    pub fn outbox_dump(&self) -> Vec<(String, i32, bool, Option<String>)> {
        self.outbox
            .lock()
            .map(|q| {
                q.iter()
                    .map(|r| {
                        (
                            r.event_id.clone(),
                            r.attempts,
                            r.published,
                            r.last_error.clone(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
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
        outgoing: &[OutboxRecord],
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
        // Конверты — в том же критическом участке, что слепок и журнал. Транзакции
        // здесь нет, но и расходиться нечему: оба мьютекса держим до конца.
        {
            use std::sync::atomic::Ordering;
            let mut q = self
                .outbox
                .lock()
                .map_err(|_| StoreError("память исходящей очереди испорчена".into()))?;
            for rec in outgoing {
                // Повтор `event_id` — не отказ, а отсутствие действия: идентификатор
                // выведен из факта, и попытка записать тот же факт дважды означает
                // повтор, а не новое событие. В БД это `ON CONFLICT DO NOTHING`.
                if q.iter().any(|r| r.event_id == rec.event_id) {
                    continue;
                }
                let id = self.next_outbox_id.fetch_add(1, Ordering::Relaxed) + 1;
                q.push(MemoryOutboxRow {
                    id,
                    event_id: rec.event_id.clone(),
                    routing_key: rec.routing_key.clone(),
                    payload: rec.payload.clone(),
                    attempts: 0,
                    published: false,
                    next_attempt_at: 0,
                    last_error: None,
                });
            }
        }
        accounts.insert(user, snap.clone());
        Ok(())
    }

    async fn claim_outbox(
        &self,
        limit: i64,
        lease_ms: i64,
    ) -> Result<Vec<OutboxClaim>, StoreError> {
        let now = Self::now_ms();
        let mut q = self
            .outbox
            .lock()
            .map_err(|_| StoreError("память исходящей очереди испорчена".into()))?;
        let mut out = Vec::new();
        for r in q.iter_mut() {
            if out.len() as i64 >= limit {
                break;
            }
            if r.published || r.next_attempt_at > now {
                continue;
            }
            r.next_attempt_at = now + lease_ms;
            out.push(OutboxClaim {
                id: r.id,
                event_id: r.event_id.clone(),
                routing_key: r.routing_key.clone(),
                payload: r.payload.clone(),
                attempts: r.attempts,
            });
        }
        Ok(out)
    }

    async fn mark_published(&self, id: i64) -> Result<(), StoreError> {
        let mut q = self
            .outbox
            .lock()
            .map_err(|_| StoreError("память исходящей очереди испорчена".into()))?;
        match q.iter_mut().find(|r| r.id == id) {
            Some(r) => {
                r.published = true;
                Ok(())
            }
            // Строки нет — это не «ну и ладно»: релей пометил бы отправленным то,
            // чего в очереди не было, и мы бы не узнали, что потеряли.
            None => Err(StoreError(format!(
                "исходящая очередь: строки №{id} нет, пометить отправленной нечего"
            ))),
        }
    }

    async fn mark_failed(
        &self,
        id: i64,
        attempts: i32,
        retry_in_ms: i64,
        error: &str,
    ) -> Result<(), StoreError> {
        let now = Self::now_ms();
        let mut q = self
            .outbox
            .lock()
            .map_err(|_| StoreError("память исходящей очереди испорчена".into()))?;
        match q.iter_mut().find(|r| r.id == id) {
            Some(r) => {
                r.attempts = attempts;
                r.next_attempt_at = now + retry_in_ms;
                r.last_error = Some(error.chars().take(500).collect());
                Ok(())
            }
            None => Err(StoreError(format!(
                "исходящая очередь: строки №{id} нет, записать отказ некуда"
            ))),
        }
    }

    async fn outbox_backlog(&self) -> Result<i64, StoreError> {
        let q = self
            .outbox
            .lock()
            .map_err(|_| StoreError("память исходящей очереди испорчена".into()))?;
        Ok(q.iter().filter(|r| !r.published).count() as i64)
    }

    /// Личностей здесь нет: `resolve_identity` у этого хранилища отказывает, а
    /// значит и обратного пути быть не может. `None`, а не выдуманная пара —
    /// адресата у события нет, и это надо увидеть, а не подменить.
    async fn identity_of(&self, _user: UserId) -> Result<Option<(String, String)>, StoreError> {
        Ok(None)
    }
}
