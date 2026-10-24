//! # gateway — шлюз терминала
//!
//! REST + WS для торгового терминала. Рыночные данные — **реальные с Binance** (ADR-013):
//! `/instruments`, `/candles`, `/depth`, `/stats` из [`feed`]; лента и live-цена — через WS `/stream`.
//! Торговля — **бумажный брокер** (ADR-014): позиции, маржа, P&L, SL/TP, лимитные ордера.
//!
//! Терминал автономный: без логина, один аккаунт [`DEFAULT_USER`]. Как его авторизовать из внешних
//! сайтов/CMS — отдельное решение (пивот 2026-07-25, см. STATUS). Доменные типы наружу не протекают.

/// Публикация событий счёта на шину платформы (TRM-40).
pub mod bus;
pub mod feed;
/// Правило видимости по юрисдикции (TRM-15, Р-058). Публичный модуль, хотя
/// вызывается сегодня только из тестов: он участник сверки реализаций между
/// репозиториями, и прятать его значило бы прятать сам факт сверки.
pub mod jurisdiction;
mod sso;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};

use domain::account::UserId;

use broker::journal::{self, Journal, Origin};
use broker::{Broker, PosSide};
use persistence::{MemoryStore, Persistence};

/// Реэкспорт: тестам и бинарю нужен трейт хранилища, чтобы подставить свою реализацию.
pub use persistence;

/// Единственный счёт автономного терминала. Интеграция с внешними сайтами/CMS определит,
/// как сюда попадает настоящая личность пользователя.
const DEFAULT_USER: UserId = UserId(1);

// ============================ Состояние ====================================

/// Кэш свечей: (инструмент, таймфрейм) → (время загрузки, свечи).
type KlineCache = HashMap<(u32, u32), (Instant, Vec<feed::Kline>)>;
/// Кэш стакана: (инструмент, запрошенная глубина) → (время, стакан).
type DepthCache = HashMap<(u32, usize), (Instant, feed::Depth)>;
/// Кэш статистики: инструмент → (время, изменения по таймфреймам).
type StatsCache = HashMap<u32, (Instant, Vec<StatDto>)>;
/// Счёт → внешняя личность `(tenant, sub)`; `None` — счёт не привязан к сайту.
type IdentityCache = HashMap<UserId, Option<(String, String)>>;

/// Таймфреймы для статистики актива.
const STAT_TFS: [u32; 8] = [60, 300, 900, 1800, 3600, 14400, 86400, 604800];

#[derive(Clone)]
pub struct AppState {
    events_tx: broadcast::Sender<String>,
    // Реальные данные (Binance):
    feed_instruments: Arc<Vec<feed::FeedInstrument>>,
    tickers: Arc<Mutex<HashMap<u32, feed::Ticker>>>,
    /// Unix-секунды последнего **принятого** сообщения Binance, 0 — не было ни одного.
    ///
    /// Меряем живость фида приходом данных, а не состоянием сокета: открытый сокет,
    /// по которому ничего не идёт, — это молчащий фид при бодром `connected`. Здоровье
    /// процесса не означает наличия связи.
    feed_last_msg: Arc<std::sync::atomic::AtomicI64>,
    /// Фид молчал дольше порога, и этот перерыв ещё не отмечен в журнале.
    ///
    /// Снимается первым же прогоном триггеров по свежим ценам: именно он идёт
    /// по РАЗОРВАННОМУ ряду (Р-073, TRM-19), и именно его записи обязаны быть
    /// отличимы. Взводится один раз на перерыв, а не держится всё время
    /// молчания: иначе «разрывом» оказался бы каждый тик простоя, и признак
    /// перестали бы читать — как перестают читать молчание.
    feed_gap_pending: Arc<std::sync::atomic::AtomicBool>,
    klines_cache: Arc<Mutex<KlineCache>>,
    depth_cache: Arc<Mutex<DepthCache>>,
    stats_cache: Arc<Mutex<StatsCache>>,
    // Бумажный брокер (ADR-014):
    broker: Arc<Mutex<Broker>>,
    // Durable-хранилище счетов (ADR-016).
    store: Arc<dyn Persistence>,
    /// Лок на счёт: сериализует «мутация → слепок → запись» по одному счёту.
    acct_locks: Arc<Mutex<HashMap<UserId, Arc<Mutex<()>>>>>,
    /// Счета, поднятые из хранилища с нарушенными инвариантами (красная линия №5).
    ///
    /// Карантин, а не стоп сервиса: одна испорченная строка не должна лишать
    /// обслуживания всех. Чтение разрешено, мутации отклоняются, состояние
    /// сохраняется **как есть** — молчаливая починка уничтожила бы улику
    /// (КОНТРАКТЫ §4в).
    quarantined: Arc<Mutex<std::collections::HashSet<UserId>>>,
    /// Журналы счетов (TRM-13): по одному на счёт, пополняются только добавлением.
    ///
    /// Лежат рядом с брокером, а не внутри него, по той же причине, что и хранилище:
    /// ядро детерминировано и не знает ни времени, ни источника события. Брокер
    /// говорит «что произошло», здесь к этому добавляется «кто и когда».
    journals: Arc<Mutex<HashMap<UserId, Journal>>>,
    /// Откуда берутся разрешённые инструменты сайта (регуляторика, ADR-023 Т3).
    ///
    /// Источник по контракту — `GET {CMS}/v1/cms/sites/{slug}`, поле `instruments`,
    /// сервер-сервер под `X-API-Key`. Поле — **плоский список символов** (Р-024):
    /// числовые id живут в `DEFS` и больше нигде, требовать их от CMS значило бы
    /// вынести нашу нумерацию в межсистемный контракт.
    ///
    /// `TenantSource::None` — источник не сконфигурирован вовсе. Это **не** «всё
    /// разрешено»: в тенантном режиме отсутствие списка закрывает торговлю.
    tenant_instruments: Arc<TenantSource>,
    /// Сопоставление «символ → внутренний id», построенное из `DEFS` (Р-024).
    ///
    /// Граница перевода стоит здесь, у нас, потому что нумерация — наша деталь.
    symbols: Arc<HashMap<String, u32>>,
    // SSO платформы (ADR-023, Т2): вход по handoff-JWT, сессии терминала.
    sso: Arc<sso::Sso>,
    /// Требовать ли сессию на торговых ручках. Выкл (dev/standalone) → счёт по умолчанию.
    sso_enabled: bool,
    /// Кеш «внутренний счёт → внешняя личность `(tenant, sub)`» (TRM-40).
    ///
    /// Событие адресуется парой «сайт + пользователь сайта»: внутренний `UserId`
    /// за границей терминала не значит ничего. Пара неизменна (её заводит
    /// `resolve_identity` один раз), поэтому кешируется без срока — включая
    /// `None`: «счёт заведён не через SSO» тоже ответ, и спрашивать его у базы
    /// на каждой сделке незачем.
    identities: Arc<Mutex<IdentityCache>>,
    /// Настроена ли шина. Отдаётся в `/health`: «очередь копится» и «очередь
    /// копится, потому что слать некуда» — разные состояния, и чинятся разными
    /// людьми.
    bus_enabled: bool,
}

pub fn build_state() -> AppState {
    build_state_with(Arc::new(MemoryStore::new()))
}

/// Собрать состояние с конкретным хранилищем. `MemoryStore` = всё в памяти процесса
/// (тесты, запуск без БД) — но путь записи при этом настоящий, см. `persistence`.
pub fn build_state_with(store: Arc<dyn Persistence>) -> AppState {
    let (events_tx, _rx) = broadcast::channel(4096);
    // SSO: JWKS платформы = {CABINET_URL}/api/sso/jwks. Без CABINET_URL SSO выключен
    // (терминал на счёте по умолчанию — dev/standalone). SSO_DISABLED=1 выключает даже
    // при заданном CABINET_URL (стек/тесты работают без реальных токенов).
    let jwks_url = std::env::var("CABINET_URL")
        .ok()
        .map(|b| b.trim_end_matches('/').to_string())
        .filter(|b| !b.is_empty())
        .map(|b| format!("{b}/api/sso/jwks"))
        .unwrap_or_default();
    let sso = Arc::new(sso::Sso::new(jwks_url));
    let sso_enabled = sso.enabled() && std::env::var("SSO_DISABLED").ok().as_deref() != Some("1");
    // Стартовый баланс демо-счёта. Дефолт $100k; в интеграции с платформой выравнивается
    // под сайт (env DEMO_START_BALANCE_CENTS; целевое — per-tenant demoStartBalanceCents из
    // CMS, когда появится server-side ключ, ADR-023).
    let start_cents = std::env::var("DEMO_START_BALANCE_CENTS")
        .ok()
        .and_then(|s| s.parse::<i128>().ok())
        .filter(|c| *c > 0)
        .unwrap_or(10_000_000);
    let feed_instruments = feed::instruments();
    let symbols = symbol_index(&feed_instruments);
    let tenant_instruments = TenantSource::from_env(&symbols);
    AppState {
        events_tx,
        feed_instruments: Arc::new(feed_instruments),
        symbols: Arc::new(symbols),
        tickers: Arc::new(Mutex::new(HashMap::new())),
        feed_last_msg: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        feed_gap_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        klines_cache: Arc::new(Mutex::new(HashMap::new())),
        depth_cache: Arc::new(Mutex::new(HashMap::new())),
        stats_cache: Arc::new(Mutex::new(HashMap::new())),
        tenant_instruments: Arc::new(tenant_instruments),
        broker: Arc::new(Mutex::new(Broker::new(start_cents, 1))), // старт из env, leverage 1
        store,
        acct_locks: Arc::new(Mutex::new(HashMap::new())),
        quarantined: Arc::new(Mutex::new(std::collections::HashSet::new())),
        journals: Arc::new(Mutex::new(HashMap::new())),
        sso,
        sso_enabled,
        identities: Arc::new(Mutex::new(HashMap::new())),
        bus_enabled: false,
    }
}

/// Запустить релей шины, если она настроена (TRM-40).
///
/// Без `BUS_URL` релей не запускается, но **очередь наполняется всё равно**: это
/// не заглушка ради зелёного теста. События копятся в хранилище и уедут, когда
/// шину настроят. Молчаливая потеря на этом месте — ровно то, чем был массив в
/// памяти.
pub fn spawn_bus_relay(st: &mut AppState) {
    match bus::publisher_from_env() {
        Some(publisher) => {
            st.bus_enabled = true;
            bus::spawn_relay(st.store.clone(), publisher);
        }
        None => {
            st.bus_enabled = false;
            eprintln!(
                "[bus] BUS_URL не задан — публикации НЕТ. События копятся в исходящей очереди и уедут после настройки; их число видно в /health (outbox_pending)."
            );
        }
    }
}

/// Поднять счета из хранилища. Вызывается один раз на старте, до приёма запросов.
pub async fn restore_state(st: &AppState) -> Result<(), persistence::StoreError> {
    st.store.init().await?;
    // Ошибка чтения поднимается наружу и роняет старт. Это **не** то же самое, что
    // «журнала нет»: недоступное хранилище и пустая лента — разные беды, и сервис,
    // который их путает, стартует на выдуманной истории (TRM-13).
    let loaded = st.store.load_all().await?;
    let accounts_n = loaded.accounts.len();
    let (lev, start_balance) = {
        let b = st.broker.lock().await;
        (b.leverage(), b.start_balance())
    };
    {
        let mut b = st.broker.lock().await;
        for (id, snap) in loaded.accounts {
            b.restore(id, snap);
        }
    }
    // Журналы: поднять, проверить цепочку, свернуть. Испорченная лента — не повод
    // «взять, что разобралось»: счёт уходит в карантин вместе с причиной.
    {
        let mut js = st.journals.lock().await;
        let mut q = st.quarantined.lock().await;
        for (user, events) in loaded.journals {
            match Journal::load(lev, start_balance, events) {
                Ok(j) => {
                    js.insert(user, j);
                }
                Err(fault) => {
                    eprintln!(
                        "[gateway] ЖУРНАЛ ИСПОРЧЕН, счёт {} в карантине: {fault}",
                        user.0
                    );
                    q.insert(user);
                }
            }
        }
    }
    // Красная линия №5. Счета, поднятые с нарушенными инвариантами, уходят в
    // карантин: читать можно, менять нельзя. Сервис при этом стартует — стоп
    // сервиса не применяется нигде (КОНТРАКТЫ §4в), потому что одна испорченная
    // строка не должна лишать обслуживания всех остальных.
    {
        let b = st.broker.lock().await;
        let js = st.journals.lock().await;
        let mut q = st.quarantined.lock().await;
        for user in b.users() {
            let snap = b.snapshot(user);
            let violations = broker::check_snapshot(&snap, b.leverage());
            if !violations.is_empty() {
                q.insert(user);
                for v in &violations {
                    eprintln!(
                        "[gateway] ИНВАРИАНТ НАРУШЕН, счёт {} в карантине: {v}",
                        user.0
                    );
                }
            }
            // Остаток обязан сходиться с журналом (TRM-13). Счёт, чью историю нечем
            // переиграть, — такой же карантин: править его деньги вслепую нельзя.
            // Чинить расхождение нельзя тем более: подогнанный слепок уничтожает улику.
            if q.contains(&user) {
                continue; // уже в карантине, причина названа
            }
            let empty = Journal::new(b.leverage(), b.start_balance());
            let status = journal::audit(js.get(&user).unwrap_or(&empty), &snap);
            if !status.ok() {
                q.insert(user);
                eprintln!(
                    "[gateway] ЖУРНАЛ НЕ СХОДИТСЯ [{}], счёт {} в карантине: {status}",
                    status.tag(),
                    user.0
                );
            }
        }
        if !q.is_empty() {
            eprintln!("[gateway] счетов в карантине: {} — мутации по ним отклоняются, состояние не чинится автоматически", q.len());
        }
    }
    if accounts_n > 0 {
        println!("[gateway] восстановлено из хранилища: счетов {accounts_n}");
    }
    Ok(())
}

/// Внешняя личность счёта для шины: `(tenant, sub)`. `None` — адресата нет.
///
/// Отказ чтения — это **не** `None`. `None` означает «счёт не привязан к сайту»
/// (standalone), и событие по нему никому не адресовано; отказ означает «мы не
/// знаем», и выдавать его за первое значило бы тихо терять события при моргнувшей
/// базе. Поэтому отказ громкий и не кешируется.
async fn identity_for_bus(st: &AppState, user: UserId) -> Option<(String, String)> {
    if let Some(found) = st.identities.lock().await.get(&user) {
        return found.clone();
    }
    match st.store.identity_of(user).await {
        Ok(found) => {
            st.identities.lock().await.insert(user, found.clone());
            found
        }
        Err(e) => {
            eprintln!(
                "[bus] личность счёта {} не прочитана, событие НЕ будет собрано: {e}",
                user.0
            );
            None
        }
    }
}

/// Собрать конверты в шину по изменению счёта (TRM-40).
///
/// Отдельной функцией, потому что путей мутации два — ручная операция и сработавший
/// триггер, — а событие о сделке обязано уходить с обоих. Проверка, стоящая на том
/// пути, о котором мы помним, пропускает ровно тот, о котором спорят: сработавший
/// стоп меняет счёт так же, как нажатая кнопка.
async fn bus_envelopes(
    st: &AppState,
    user: UserId,
    before: bus::Before,
    snap: &broker::AccountSnapshot,
    kinds: &[broker::journal::EventKind],
    sealed: &[broker::journal::AccountEvent],
) -> Vec<persistence::OutboxRecord> {
    if kinds.is_empty() {
        return Vec::new();
    }
    let Some((tenant, sub)) = identity_for_bus(st, user).await else {
        return Vec::new();
    };
    let Some(last) = sealed.last() else {
        return Vec::new();
    };
    let who = bus::Addressee {
        tenant: &tenant,
        sub: &sub,
        user: user.0,
        seq: last.seq(),
        at_ms: last.at_ms(),
    };
    let by_id = |id: u32| feed_by_id(st, id);
    bus::envelopes_for(&who, &before, snap, kinds, &by_id)
        .iter()
        .map(|e| e.to_record())
        .collect()
}

/// Лок конкретного счёта (создаётся лениво).
async fn account_lock(st: &AppState, user: UserId) -> Arc<Mutex<()>> {
    st.acct_locks
        .lock()
        .await
        .entry(user)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Мутация счёта с сохранением (ADR-016) и записью в журнал (TRM-13).
///
/// Порядок жёсткий, и каждый шаг стоит на своём месте:
///
/// 1. под локом счёта мутировать брокер, забрать его события и снять слепок;
/// 2. проверить инварианты слепка (красная линия №5) — отказ до записи;
/// 3. запечатать события и **свериться**: совпадает ли состояние, которое утверждает
///    журнал, с тем, что в слепке. Сверка до записи, потому что запись не отменить:
///    лента только дописывается, и ошибочную запись оттуда уже не убрать;
/// 4. слепок и записи журнала — одной транзакцией хранилища;
/// 5. только теперь пополнить журнал в памяти.
///
/// `origin` — кто источник: [`Origin::Client`] для действий из терминала,
/// [`Origin::System`] для триггеров, [`Origin::Operator`] для ручной проводки (ADM-03).
async fn with_account<T>(
    st: &AppState,
    user: UserId,
    origin: Origin,
    f: impl FnOnce(&mut Broker) -> Result<T, ApiErr>,
) -> Result<T, ApiErr> {
    // Счёт в карантине не мутируем: его состояние уже неверно, и любая операция
    // поверх него закрепила бы порчу. Читать при этом можно.
    if st.quarantined.lock().await.contains(&user) {
        return Err(err(StatusCode::LOCKED, "account_quarantined"));
    }

    let lock = account_lock(st, user).await;
    let _guard = lock.lock().await;

    let (out, before, snap, kinds, lev, start_balance) = {
        let mut b = st.broker.lock().await;
        let before = b.snapshot(user);
        let out = f(&mut b);
        // Очередь сливается ВСЕГДА, в том числе на отказе: события несостоявшейся
        // операции не должны уехать в журнал прицепом к следующей.
        let kinds = b.take_events(user);
        let snap = b.snapshot(user);
        (out, before, snap, kinds, b.leverage(), b.start_balance())
    };
    let out = out?;

    // Красная линия №5, проверка ДО записи: если мутация сделала бы состояние
    // неверным, операция отклоняется и изменение не фиксируется. Граница отказа
    // проходит по счёту — соседи не затронуты, сервис не останавливается.
    // Код отдельный: это не деловой отказ вроде «недостаточно средств», и путать
    // их нельзя (КОНТРАКТЫ §4в).
    let violations = broker::check_snapshot(&snap, lev);
    if !violations.is_empty() {
        st.broker.lock().await.restore(user, before);
        for v in &violations {
            eprintln!(
                "[gateway] ИНВАРИАНТ НАРУШЕН операцией, счёт {} откачен: {v}",
                user.0
            );
        }
        return Err(err(StatusCode::UNPROCESSABLE_ENTITY, "invariant_violated"));
    }

    // Журнал (TRM-13). Запечатываем, но ещё не пишем: сначала сверка.
    let (sealed, before_bus) = {
        let mut js = st.journals.lock().await;
        let j = js
            .entry(user)
            .or_insert_with(|| Journal::new(lev, start_balance));
        // Чем счёт был до операции — по свёртке журнала (TRM-40). Берём здесь, пока
        // запись ещё не принята: после `commit_journal` свёртка уже «после».
        let before_bus = bus::Before {
            closed: j.state().closed.len(),
            balance: j.state().balance,
        };
        let sealed = j.seal(now_ms(), &origin, &kinds);
        let would_be = j.preview(&sealed);
        let diverged = journal::reconcile(&would_be, &snap);
        if !diverged.is_empty() {
            drop(js);
            st.broker.lock().await.restore(user, before);
            for d in &diverged {
                eprintln!(
                    "[gateway] ЖУРНАЛ НЕ СХОДИТСЯ, операция по счёту {} откачена: {d}",
                    user.0
                );
            }
            return Err(err(StatusCode::UNPROCESSABLE_ENTITY, "journal_divergence"));
        }
        (sealed, before_bus)
    };

    // Конверты в шину — ДО записи, отправка — ПОСЛЕ (TRM-40). Собираем здесь,
    // чтобы они уехали в хранилище той же транзакцией, что слепок и журнал:
    // событие, записанное отдельно от денег, рано или поздно с ними разойдётся.
    let outgoing = bus_envelopes(st, user, before_bus, &snap, &kinds, &sealed).await;

    if let Err(e) = st
        .store
        .save_account(user, &snap, &sealed, &outgoing)
        .await
    {
        st.broker.lock().await.restore(user, before);
        eprintln!("[gateway] запись состояния не удалась, счёт откачен: {e}");
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable"));
    }

    // Записано — теперь пополняем ленту в памяти. Ошибка здесь означала бы, что
    // журнал в памяти разошёлся с журналом в хранилище: это карантин, а не предупреждение.
    commit_journal(st, user, sealed).await;
    Ok(out)
}

/// Принять в память уже записанные в хранилище записи.
async fn commit_journal(st: &AppState, user: UserId, sealed: Vec<broker::journal::AccountEvent>) {
    let mut js = st.journals.lock().await;
    let lev_start = {
        let b = st.broker.lock().await;
        (b.leverage(), b.start_balance())
    };
    let j = js
        .entry(user)
        .or_insert_with(|| Journal::new(lev_start.0, lev_start.1));
    for e in sealed {
        if let Err(fault) = j.commit(e) {
            eprintln!(
                "[gateway] ЖУРНАЛ В ПАМЯТИ РАЗОШЁЛСЯ С ХРАНИЛИЩЕМ, счёт {} в карантине: {fault}",
                user.0
            );
            st.quarantined.lock().await.insert(user);
            return;
        }
    }
}

impl AppState {
    /// Отметить, что от Binance пришло опознанное сообщение (см. `feed_last_msg`).
    pub(crate) fn mark_feed_alive(&self) {
        self.feed_last_msg
            .store(now_secs(), std::sync::atomic::Ordering::Relaxed);
    }

    /// Разорван ли ряд цены к этому прогону триггеров (Р-073, TRM-19).
    ///
    /// Пока фид молчит дольше порога — взводим признак и отвечаем `false`:
    /// цены старые, новых событий они не породят, и помечать нечего. Как
    /// только фид ожил, ПЕРВЫЙ прогон получает `true` и признак гаснет — он
    /// и идёт по разорванному ряду. Дальше ряд снова непрерывен.
    ///
    /// Порог один с `/health`: два числа для одного вопроса однажды разойдутся,
    /// и тогда здоровье и журнал будут рассказывать разное об одном молчании.
    pub(crate) fn note_feed_gap(&self) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        let last = self.feed_last_msg.load(Relaxed);
        let stale = last == 0 || now_secs() - last > FEED_STALE_AFTER_SECS;
        if stale {
            self.feed_gap_pending.store(true, Relaxed);
            return false;
        }
        self.feed_gap_pending.swap(false, Relaxed)
    }
}

// ================= Allow-list инструментов: символы → внутренние id ==========
//
// Решение штаба Р-024. Контракт между CMS, MDS, витриной, кабинетом и нами —
// **символы**; числовые id живут в `feed::DEFS` и больше нигде. Требовать от трёх
// систем знать нашу нумерацию значило бы вынести реализацию терминала в
// межсистемный контракт, а перенумерация внутри терминала молча изменила бы,
// кому что разрешено торговать. Поэтому перевод стоит здесь, на нашей границе.

/// Нормализованное написание символа: верхний регистр, без разделителей.
///
/// Нужно потому, что одну и ту же пару называют по-разному: MDS — `BTCUSD`,
/// Binance и мы внутри — `BTCUSDT`, витрина показывает `BTC-USDT`. Нормализация
/// снимает только **оформление** (регистр, `-`, `/`, `_`, пробелы); разные
/// написания самого имени разводит `symbol_index`, и делает это явным списком.
fn normalize_symbol(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_uppercase())
        .collect()
}

/// Индекс «символ → внутренний id» по списку инструментов фида.
///
/// Каждый инструмент отзывается на два написания:
///
/// * собственное, Binance-овское: `BTCUSDT` (оно же `BTC-USDT` после нормализации);
/// * каноническое для платформы, как его хранит CMS и отдаёт MDS: `BTCUSD`.
///
/// Второе — не догадка. Проверено по живому MDS контура 06.10: все 30 пар `DEFS`
/// присутствуют в его вселенной, у каждой `providerSymbol` совпадает с нашим
/// Binance-символом, а канонический символ ровно равен «база + USD». То есть
/// `BTCUSD` и `BTCUSDT` — один и тот же рынок у одного и того же провайдера,
/// и это утверждение источника, а не наша подгонка строк.
///
/// Коллизий быть не должно: если два инструмента отзовутся на одно написание,
/// разрешение «поплывёт» молча. Ловится тестом `symbol_index_has_no_collisions`.
fn symbol_index(instruments: &[feed::FeedInstrument]) -> HashMap<String, u32> {
    let mut out: HashMap<String, u32> = HashMap::new();
    for f in instruments {
        let binance = normalize_symbol(&f.binance);
        let mut spellings = vec![binance.clone(), normalize_symbol(&f.symbol)];
        if let Some(base) = binance.strip_suffix("USDT") {
            spellings.push(format!("{base}USD"));
        }
        for s in spellings {
            match out.insert(s.clone(), f.id) {
                Some(other) if other != f.id => {
                    // Не `unwrap`/`panic` в рантайме: падать на старте из-за
                    // справочника — это стоп сервиса, который у нас не применяется
                    // нигде (КОНТРАКТЫ §4в). Но и молчать нельзя.
                    eprintln!(
                        "[gateway] КОЛЛИЗИЯ СИМВОЛОВ: «{s}» указывает и на {other}, и на {}; \
                         разрешения по этому символу недостоверны",
                        f.id
                    );
                }
                _ => {}
            }
        }
    }
    out
}

/// Список сайта после сопоставления символов внутренним id.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TenantAllowList {
    /// Список получен и сопоставлен целиком. Пустой вектор — валидное значение
    /// «сайту не разрешено ничего» (Р-025), а не «фильтра нет».
    Resolved(Vec<u32>),
    /// В списке есть символ, которого терминал не знает.
    ///
    /// Выбросить его молча нельзя: разрешённое значение исчезло бы из прав, и
    /// **никто бы об этом не узнал** — тихое сужение прав на регуляторной границе
    /// (Р-024). Поэтому отказ, а не пропуск, и отказ на весь список сайта: частично
    /// сопоставленный список — это уже не тот список, который прислал источник.
    Unknown(Vec<String>),
}

impl TenantAllowList {
    /// Сопоставить присланные символы внутренним id.
    fn resolve(index: &HashMap<String, u32>, symbols: &[String]) -> Self {
        let mut ids = Vec::with_capacity(symbols.len());
        let mut unknown = Vec::new();
        for s in symbols {
            match index.get(&normalize_symbol(s)) {
                Some(id) => ids.push(*id),
                None => unknown.push(s.clone()),
            }
        }
        if unknown.is_empty() {
            ids.sort_unstable();
            ids.dedup();
            TenantAllowList::Resolved(ids)
        } else {
            TenantAllowList::Unknown(unknown)
        }
    }
}

/// Почему списка нет. Коды наружу разные: причины чинятся по-разному (§4б).
enum NoList {
    /// Источник не ответил, ответил отказом или не сказал про инструменты вовсе.
    Unavailable,
    /// Источник ответил, но список содержит неизвестные символы.
    Unreadable,
}

impl NoList {
    fn api(&self) -> ApiErr {
        match self {
            // 503, а не 403: список не «сказал нет», его просто нет.
            NoList::Unavailable => err(
                StatusCode::SERVICE_UNAVAILABLE,
                "tenant_instruments_unavailable",
            ),
            // Тоже закрыто, но чинится правкой списка в CMS, а не связью.
            NoList::Unreadable => err(
                StatusCode::SERVICE_UNAVAILABLE,
                "tenant_instruments_unreadable",
            ),
        }
    }
}

/// Откуда терминал берёт allow-list сайта.
///
/// Порядок выбора объявляется в логе на старте: источник, выбранный молча, —
/// это источник, о котором никто не знает.
enum TenantSource {
    /// Живая CMS: `GET {base}/cms/sites/{slug}` под `X-API-Key`, сервер-сервер.
    ///
    /// Читаем **на каждую проверку границы**, без кеша: торговая граница читает
    /// конфиг живым (Р-030), а бессрочного last-good здесь не будет — список
    /// меняется ровно потому, что меняются разрешения (§4б).
    Cms {
        base: String,
        api_key: String,
        http: reqwest::Client,
    },
    /// Статический список из окружения — для разработки и офлайна.
    Static(HashMap<String, TenantAllowList>),
    /// Источника нет. В тенантном режиме это закрытая торговля, а не «всё можно».
    None,
}

impl TenantSource {
    fn from_env(index: &HashMap<String, u32>) -> Self {
        let base = std::env::var("CMS_BASE_URL")
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let api_key = std::env::var("CMS_API_KEY").unwrap_or_default();
        if !base.is_empty() && !api_key.is_empty() {
            println!("[gateway] allow-list: источник — CMS {base}/cms/sites/{{slug}} (сервер-сервер, X-API-Key)");
            let http = reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap_or_default();
            return TenantSource::Cms {
                base,
                api_key,
                http,
            };
        }
        if !base.is_empty() || !api_key.is_empty() {
            eprintln!("[gateway] allow-list: CMS сконфигурирована наполовину (нужны и CMS_BASE_URL, и CMS_API_KEY) — источник CMS не используется");
        }
        match parse_tenant_instruments(std::env::var("TENANT_INSTRUMENTS").ok().as_deref(), index) {
            Some(map) => {
                println!(
                    "[gateway] allow-list: источник — TENANT_INSTRUMENTS, сайтов {}",
                    map.len()
                );
                TenantSource::Static(map)
            }
            None => {
                println!("[gateway] allow-list: источника нет — в тенантном режиме торговля закрыта (fail-closed)");
                TenantSource::None
            }
        }
    }

    /// Список сайта. Любой отказ закрывает торговлю и называет причину в логе.
    async fn allow_list(
        &self,
        site: &str,
        index: &HashMap<String, u32>,
    ) -> Result<Vec<u32>, NoList> {
        let list = match self {
            TenantSource::None => {
                eprintln!(
                    "[gateway] {site}: источник allow-list не сконфигурирован — торговля закрыта"
                );
                return Err(NoList::Unavailable);
            }
            TenantSource::Static(map) => match map.get(site) {
                Some(l) => l.clone(),
                None => {
                    eprintln!("[gateway] {site}: сайта нет в allow-list — торговля закрыта");
                    return Err(NoList::Unavailable);
                }
            },
            TenantSource::Cms {
                base,
                api_key,
                http,
            } => {
                let url = format!("{base}/cms/sites/{site}");
                let resp = http.get(&url).header("X-API-Key", api_key).send().await;
                let resp = match resp {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("[gateway] {site}: CMS недоступна ({e}) — торговля закрыта");
                        return Err(NoList::Unavailable);
                    }
                };
                if !resp.status().is_success() {
                    // Разные причины, один исход: списка нет. В логе причина видна,
                    // чтобы «ключ не приняли» не чинили как «сайта не существует».
                    eprintln!(
                        "[gateway] {site}: CMS ответила {} — торговля закрыта",
                        resp.status()
                    );
                    return Err(NoList::Unavailable);
                }
                let body: SiteConfigDto = match resp.json().await {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!(
                            "[gateway] {site}: ответ CMS не разобран ({e}) — торговля закрыта"
                        );
                        return Err(NoList::Unavailable);
                    }
                };
                match body.instruments {
                    // Отсутствие поля — это «источник ничего не сказал», а не
                    // «ничего не разрешено» и тем более не «разрешено всё».
                    None => {
                        eprintln!("[gateway] {site}: в ответе CMS нет поля instruments — список неизвестен, торговля закрыта");
                        return Err(NoList::Unavailable);
                    }
                    Some(symbols) => TenantAllowList::resolve(index, &symbols),
                }
            }
        };
        match list {
            TenantAllowList::Resolved(ids) => Ok(ids),
            TenantAllowList::Unknown(unknown) => {
                eprintln!(
                    "[gateway] {site}: allow-list содержит неизвестные терминалу символы [{}] — \
                     список не принят целиком, торговля закрыта. Молча выбросить их значило бы \
                     тихо сузить права (Р-024)",
                    unknown.join(", ")
                );
                Err(NoList::Unreadable)
            }
        }
    }
}

/// Ответ `GET /v1/cms/sites/{slug}`. Разбираем мягко: неизвестные поля игнорируем,
/// но **отсутствие** `instruments` отличаем от пустого списка — это разные
/// утверждения («не сказали» против «не разрешено ничего»).
#[derive(Deserialize)]
struct SiteConfigDto {
    #[serde(default)]
    instruments: Option<Vec<String>>,
}

/// Разобрать статический источник allow-list: `site:BTCUSD,ETHUSD;other:XRPUSD`.
///
/// `None` — переменная не задана (источника нет). Пустой список у сайта — валидное
/// значение и означает «сайту не разрешено ничего», а не «фильтра нет».
fn parse_tenant_instruments(
    raw: Option<&str>,
    index: &HashMap<String, u32>,
) -> Option<HashMap<String, TenantAllowList>> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let mut out: HashMap<String, TenantAllowList> = HashMap::new();
    for site_part in raw.split(';').filter(|p| !p.trim().is_empty()) {
        let (site, symbols) = site_part.split_once(':').unwrap_or((site_part, ""));
        let symbols: Vec<String> = symbols
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        out.insert(
            site.trim().to_string(),
            TenantAllowList::resolve(index, &symbols),
        );
    }
    Some(out)
}

/// Кто выполняет запрос. `tenant` есть только в тенантном режиме (вход по SSO);
/// в standalone его нет — и юрисдикции, чей allow-list надо уважать, тоже нет.
struct Principal {
    user: UserId,
    tenant: Option<String>,
}

/// Можно ли этому принципалу **открывать позицию** по инструменту.
///
/// Граница проходит здесь, а не на выдаче списка: спрятать инструмент из витрины
/// недостаточно, торговать им прямым запросом всё равно можно было бы.
///
/// Fail-closed (решение штаба, КОНТРАКТЫ §4б): нет конфига сайта — торговли нет.
/// Разные юрисдикции разрешают разное, и отказ инфраструктуры не должен
/// превращаться в разрешение. Отличаем «списка нет» от «в списке нет»: обе ветки
/// запрещают, но чинятся по-разному, и человек должен видеть причину.
async fn ensure_can_open(st: &AppState, who: &Principal, instrument: u32) -> Result<(), ApiErr> {
    let Some(tenant) = who.tenant.as_deref() else {
        return Ok(()); // standalone: сайта нет, ограничивать нечем и не от чьего имени
    };
    let allowed = st
        .tenant_instruments
        .allow_list(tenant, &st.symbols)
        .await
        .map_err(|e| e.api())?;
    if allowed.contains(&instrument) {
        Ok(())
    } else {
        Err(err(StatusCode::FORBIDDEN, "instrument_not_allowed"))
    }
}

/// Текущие марк-цены (инструмент → last) из фида — для P&L/equity брокера.
async fn marks(st: &AppState) -> HashMap<u32, i64> {
    st.tickers
        .lock()
        .await
        .iter()
        .map(|(id, t)| (*id, t.last))
        .collect()
}

fn feed_by_id(st: &AppState, id: u32) -> Option<feed::FeedInstrument> {
    st.feed_instruments.iter().find(|f| f.id == id).cloned()
}

/// `Authorization: Bearer <token>` → сам токен.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// Кто выполняет запрос (ADR-023, Т2). SSO включён — из bearer-сессии, иначе `401`;
/// SSO выключен (dev/standalone) — счёт по умолчанию и никакого сайта.
async fn require_principal(st: &AppState, headers: &HeaderMap) -> Result<Principal, ApiErr> {
    if !st.sso_enabled {
        return Ok(Principal {
            user: DEFAULT_USER,
            tenant: None,
        });
    }
    let token = bearer(headers).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "missing session"))?;
    let (user, tenant) = st
        .sso
        .resolve(token)
        .await
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "invalid session"))?;
    Ok(Principal {
        user,
        tenant: Some(tenant),
    })
}

/// То же, когда нужен только счёт: чтение позиций/истории сайтом не ограничивается.
async fn require_user(st: &AppState, headers: &HeaderMap) -> Result<UserId, ApiErr> {
    Ok(require_principal(st, headers).await?.user)
}

// ============================ Роутер =======================================

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/instruments", get(list_instruments))
        .route("/candles/:instrument", get(get_candles))
        .route("/depth/:instrument", get(get_depth))
        .route("/stats/:instrument", get(get_stats))
        .route("/deals", post(open_deal).get(list_deals))
        .route("/deals/closed", get(list_closed))
        .route("/deals/:id/close", post(close_deal))
        .route("/pending", post(place_pending).get(list_pending))
        .route("/pending/:id", delete(cancel_pending))
        .route("/account", get(account))
        .route("/account/journal", get(account_journal))
        .route("/stream", get(ws_stream))
        // SSO платформы (ADR-023, Т2): обмен handoff-JWT на сессию терминала и логаут.
        .route("/v1/session", post(session_create))
        .route("/v1/session/logout", post(session_logout))
        .with_state(state)
}

// ============================ DTO ==========================================

#[derive(Serialize)]
struct ErrResp {
    error: String,
}
type ApiErr = (StatusCode, Json<ErrResp>);
fn err(code: StatusCode, msg: &str) -> ApiErr {
    (
        code,
        Json(ErrResp {
            error: msg.to_string(),
        }),
    )
}

#[derive(Deserialize)]
struct CandleQuery {
    tf: Option<u32>,
    limit: Option<usize>,
}
#[derive(Serialize)]
struct CandleDto {
    time: i64,
    open: i64,
    high: i64,
    low: i64,
    close: i64,
    volume: i64,
}
#[derive(Deserialize)]
struct DealReq {
    instrument: u32,
    side: String,
    qty: i64,
    sl: Option<i64>,
    tp: Option<i64>,
}
#[derive(Serialize)]
struct DealDto {
    id: u64,
    instrument: u32,
    symbol: String,
    side: String,
    qty: i64,
    entry: i64,
    mark: i64,
    pnl: i128,
    sl: Option<i64>,
    tp: Option<i64>,
    price_decimals: u8,
    qty_decimals: u8,
}
#[derive(Deserialize)]
struct PendingReq {
    instrument: u32,
    side: String,
    qty: i64,
    price: i64,
    sl: Option<i64>,
    tp: Option<i64>,
}
#[derive(Serialize)]
struct PendingDto {
    id: u64,
    instrument: u32,
    symbol: String,
    side: String,
    qty: i64,
    price: i64,
    sl: Option<i64>,
    tp: Option<i64>,
    price_decimals: u8,
    qty_decimals: u8,
}
#[derive(Serialize)]
struct ClosedDto {
    instrument: u32,
    symbol: String,
    side: String,
    qty: i64,
    entry: i64,
    /// Цена ИСПОЛНЕНИЯ — по ней посчитаны деньги.
    exit: i64,
    pnl: i128,
    price_decimals: u8,
    qty_decimals: u8,
    /// Уровень триггера, который ставил человек (TRM-38). Разница с `exit` и
    /// есть **проскальзывание** — показать его может только тот, у кого обе
    /// величины. Поля нет, когда закрытие не было по уровню: рыночное, по
    /// команде, неттинг.
    ///
    /// Прежде наружу уходила одна `exit`. В журнале уровень был, но журнал
    /// читает владелец счёта, а проекции — другие потребители: **одна
    /// величина, записанная в одном месте, не становится доступной во всех**.
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<i64>,
    /// Закрытие на разорванном ряду цены (Р-073). Трёхзначно: `true` — разрыв
    /// был, `false` — не было, **поля нет — неизвестно**. Последнее стоит у
    /// сделок, закрытых до появления признака: `false` там утверждал бы, что
    /// разрыва не было, то есть отвечал бы за прошлое, которого не видел.
    #[serde(skip_serializing_if = "Option::is_none")]
    across_gap: Option<bool>,
}
#[derive(Serialize)]
struct AccountDto {
    /// Модель ведения позиций счёта (ADR-025): `hedging` | `netting`. Отдаётся
    /// наружу, потому что она меняет смысл каждой встречной сделки, и человек
    /// обязан видеть её ДО того, как не сойдётся баланс.
    model: &'static str,
    balance: i128,
    equity: i128,
    used_margin: i128,
    free_margin: i128,
    open_pnl: i128,
}
#[derive(Serialize)]
struct InstrumentDto {
    id: u32,
    symbol: String,
    base: u32,
    quote: u32,
    price_decimals: u8,
    qty_decimals: u8,
    last: Option<i64>,
    bid: Option<i64>,
    ask: Option<i64>,
    high: Option<i64>,
    /// `None` — котировки по инструменту ещё нет, изменение **неизвестно**.
    ///
    /// Было `f64` с `unwrap_or(0.0)`: инструмент без тикера отдавал «0%», и
    /// витрина рисовала зелёное «+0.00%» — «рынок стоял» вместо «мы не знаем».
    /// Соседние поля на той же строке (`last`, `bid`, `ask`, `high`) всё это
    /// время были честными `Option`, и врало ровно одно поле из пяти.
    ///
    /// То же решение, что у `/stats`, где `change` обнуляем с самого начала:
    /// два ответа одного сервиса про одну величину обязаны говорить одинаково.
    change: Option<f64>,
}
#[derive(Serialize)]
struct DepthResp {
    price_decimals: u8,
    qty_decimals: u8,
    bids: Vec<(i64, i64)>,
    asks: Vec<(i64, i64)>,
}
/// Глубина стакана, запрошенная клиентом (зависит от его шага группировки).
#[derive(Deserialize)]
struct DepthQuery {
    limit: Option<usize>,
}
/// Сайт для фильтрации витрины инструментов (`?site=`).
#[derive(Deserialize)]
struct SiteQuery {
    site: Option<String>,
}
#[derive(Serialize, Clone)]
struct StatDto {
    tf: u32,
    /// `None` — посчитать не удалось (в JSON `null`), а не «изменение 0 %».
    change: Option<f64>,
}

// ============================ Хендлеры =====================================

/// Состояние сервиса. Поля `storage`/`durable` объявляют режим хранилища машине:
/// «состояние живёт только в памяти» не должно быть видно лишь в логе gateway, иначе
/// после рестарта пустой счёт выглядит как настоящий (BUG-011, КОНТРАКТЫ §5в).
///
/// Про фид сознательно **не** сообщается состояние сокета: открытое соединение, по
/// которому не приходит ничего, — это молчащий фид, а не живой. `feed_age_secs` —
/// возраст последнего принятого сообщения Binance, `feed_fresh` — свежее ли оно
/// порога. `null` в возрасте означает «не приняли ещё ни одного сообщения».
#[derive(Serialize)]
struct HealthDto {
    status: &'static str,
    storage: &'static str,
    durable: bool,
    sso: bool,
    feed_fresh: bool,
    feed_age_secs: Option<i64>,
    priced_instruments: usize,
    /// Включена ли публикация на шину (TRM-40): есть ли у релея адрес.
    bus: bool,
    /// Сколько конвертов ждёт отправки. Непустая очередь при молчащей шине —
    /// видимое состояние, а не тишина: по этому числу отличают «нечего слать»
    /// от «слать некуда». `None` — очередь не прочиталась, и это тоже ответ.
    outbox_pending: Option<i64>,
}

/// Насколько старым может быть последнее сообщение Binance, чтобы фид считался живым.
/// Топ-30 пар дают сделки непрерывно; 30 с молчания — это уже не «тихий рынок».
const FEED_STALE_AFTER_SECS: i64 = 30;

async fn health(State(st): State<AppState>) -> Json<HealthDto> {
    let last = st.feed_last_msg.load(std::sync::atomic::Ordering::Relaxed);
    let age = (last > 0).then(|| now_secs() - last);
    Json(HealthDto {
        status: "ok",
        storage: st.store.kind(),
        durable: st.store.durable(),
        sso: st.sso_enabled,
        feed_fresh: age.is_some_and(|a| a <= FEED_STALE_AFTER_SECS),
        feed_age_secs: age,
        priced_instruments: st.tickers.lock().await.len(),
        bus: st.bus_enabled,
        outbox_pending: st.store.outbox_backlog().await.ok(),
    })
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Время для записи журнала. Живёт здесь, на границе: ядро детерминировано и часов
/// не знает (красная линия №2), «когда» приходит вместе с событием снаружи.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Список инструментов с реальными котировками (Binance @ticker).
///
/// Витрина: фильтруется по `?site=`, если он задан. Это удобство, а не защита —
/// защита стоит на пути открытия сделки (`ensure_can_open`), потому что `?site=`
/// из браузера подставит кто угодно. Отказ отдаётся кодом, а не пустым списком:
/// пустой список читается как «сайту нечем торговать», что неотличимо от
/// «конфиг сайта не доехал» (BUG-011).
async fn list_instruments(
    State(st): State<AppState>,
    Query(q): Query<SiteQuery>,
) -> Result<Json<Vec<InstrumentDto>>, ApiErr> {
    let allowed: Option<Vec<u32>> = match q.site.as_deref().filter(|s| !s.is_empty()) {
        None => None,
        Some(site) => {
            let list = st
                .tenant_instruments
                .allow_list(site, &st.symbols)
                .await
                .map_err(|e| e.api())?;
            if list.is_empty() {
                // «Пустой список инструментов не отдаётся никогда» (КОНТРАКТЫ §4б).
                // Сайту действительно ничего не разрешено (Р-025) — но пустой массив
                // в ответе неотличим от «конфиг не доехал», а исчезновение витрины
                // без объяснения запрещено. Поэтому окончательное «нет» кодом.
                eprintln!(
                    "[gateway] {site}: allow-list пуст — сайту не разрешён ни один инструмент"
                );
                return Err(err(StatusCode::FORBIDDEN, "no_instruments_allowed"));
            }
            Some(list)
        }
    };
    let tickers = st.tickers.lock().await;
    let out = st
        .feed_instruments
        .iter()
        .filter(|f| allowed.as_ref().is_none_or(|a| a.contains(&f.id)))
        .map(|f| {
            let t = tickers.get(&f.id);
            InstrumentDto {
                id: f.id,
                symbol: f.symbol.clone(),
                base: f.base,
                quote: f.quote,
                price_decimals: f.price_decimals,
                qty_decimals: f.qty_decimals,
                last: t.map(|t| t.last),
                bid: t.map(|t| t.bid),
                ask: t.map(|t| t.ask),
                high: t.map(|t| t.high),
                change: t.map(|t| t.change),
            }
        })
        .collect();
    Ok(Json(out))
}

/// Реальные свечи с Binance REST (кэш ~2с).
///
/// Отказ отдаётся кодом, а не пустым массивом: `[]` читается как «рынок пуст» и
/// стирает график, хотя означал «мы не смогли получить данные» (BUG-011).
async fn get_candles(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    Query(q): Query<CandleQuery>,
) -> Result<Json<Vec<CandleDto>>, ApiErr> {
    let tf = q.tf.unwrap_or(3600);
    let limit = q.limit.unwrap_or(300).min(1000);
    let fi = feed_by_id(&st, id).ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown instrument"))?;
    let key = (id, tf);
    {
        let cache = st.klines_cache.lock().await;
        if let Some((t, v)) = cache.get(&key) {
            if t.elapsed() < Duration::from_secs(2) {
                return Ok(Json(tail_dto(v, limit)));
            }
        }
    }
    match feed::fetch_klines(
        &fi.binance,
        feed::interval(tf),
        500,
        fi.price_decimals,
        fi.qty_decimals,
    )
    .await
    {
        Ok(v) => {
            let out = tail_dto(&v, limit);
            st.klines_cache
                .lock()
                .await
                .insert(key, (Instant::now(), v));
            Ok(Json(out))
        }
        Err(e) => {
            eprintln!("[gateway] свечи {} tf={tf} недоступны: {e}", fi.symbol);
            Err(err(StatusCode::SERVICE_UNAVAILABLE, "candles unavailable"))
        }
    }
}
fn tail_dto(v: &[feed::Kline], limit: usize) -> Vec<CandleDto> {
    v[v.len().saturating_sub(limit)..]
        .iter()
        .map(|k| CandleDto {
            time: k.time,
            open: k.open,
            high: k.high,
            low: k.low,
            close: k.close,
            volume: k.volume,
        })
        .collect()
}

/// Стакан (order book) с Binance, кэш ~400мс.
///
/// Как и свечи: отказ — кодом, а не пустым стаканом. Пустой стакан означает «нет
/// заявок» — совсем другое утверждение, чем «мы не дошли до биржи» (BUG-011).
async fn get_depth(
    State(st): State<AppState>,
    Path(id): Path<u32>,
    Query(q): Query<DepthQuery>,
) -> Result<Json<DepthResp>, ApiErr> {
    let fi = feed_by_id(&st, id).ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown instrument"))?;
    // Клиент просит глубину под свой шаг группировки; Binance принимает фиксированный набор.
    let limit = match q.limit.unwrap_or(20) {
        0..=20 => 20,
        21..=50 => 50,
        51..=100 => 100,
        101..=500 => 500,
        _ => 1000,
    };
    {
        let cache = st.depth_cache.lock().await;
        if let Some((t, d)) = cache.get(&(id, limit)) {
            if t.elapsed() < Duration::from_millis(400) {
                return Ok(Json(DepthResp {
                    price_decimals: fi.price_decimals,
                    qty_decimals: fi.qty_decimals,
                    bids: d.bids.clone(),
                    asks: d.asks.clone(),
                }));
            }
        }
    }
    match feed::fetch_depth(&fi.binance, limit, fi.price_decimals, fi.qty_decimals).await {
        Ok(d) => {
            let resp = DepthResp {
                price_decimals: fi.price_decimals,
                qty_decimals: fi.qty_decimals,
                bids: d.bids.clone(),
                asks: d.asks.clone(),
            };
            st.depth_cache
                .lock()
                .await
                .insert((id, limit), (Instant::now(), d));
            Ok(Json(resp))
        }
        Err(e) => {
            eprintln!("[gateway] стакан {} недоступен: {e}", fi.symbol);
            Err(err(StatusCode::SERVICE_UNAVAILABLE, "depth unavailable"))
        }
    }
}

/// Статистика актива: изменение (%) по таймфреймам, кэш ~3с.
///
/// `change: null` — посчитать не удалось. Раньше отказ превращался в `0.0`, то есть
/// в утверждение «рынок стоял» (BUG-011); отличать это обязан клиент, а не сервер.
async fn get_stats(
    State(st): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Vec<StatDto>>, ApiErr> {
    let fi = feed_by_id(&st, id).ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown instrument"))?;
    {
        let cache = st.stats_cache.lock().await;
        if let Some((t, v)) = cache.get(&id) {
            if t.elapsed() < Duration::from_secs(3) {
                return Ok(Json(v.clone()));
            }
        }
    }
    let futs = STAT_TFS.iter().map(|&tf| {
        let sym = fi.binance.clone();
        let label = fi.symbol.clone();
        async move {
            match feed::fetch_change(&sym, feed::interval(tf)).await {
                Ok(change) => StatDto {
                    tf,
                    change: Some(change),
                },
                Err(e) => {
                    eprintln!("[gateway] статистика {label} tf={tf} недоступна: {e}");
                    StatDto { tf, change: None }
                }
            }
        }
    });
    let out: Vec<StatDto> = futures_util::future::join_all(futs).await;
    st.stats_cache
        .lock()
        .await
        .insert(id, (Instant::now(), out.clone()));
    Ok(Json(out))
}

/// Открыть сделку по текущей рыночной цене (buy=ask, sell=bid).
async fn open_deal(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<DealReq>,
) -> Result<Json<DealDto>, ApiErr> {
    let who = require_principal(&st, &headers).await?;
    ensure_can_open(&st, &who, req.instrument).await?;
    let user = who.user;
    let side = match req.side.as_str() {
        "buy" => PosSide::Long,
        "sell" => PosSide::Short,
        _ => return Err(err(StatusCode::BAD_REQUEST, "side must be buy|sell")),
    };
    if req.qty <= 0 {
        return Err(err(StatusCode::BAD_REQUEST, "qty must be > 0"));
    }
    let fi = feed_by_id(&st, req.instrument)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown instrument"))?;
    let (entry, mark) = {
        let tickers = st.tickers.lock().await;
        let t = tickers
            .get(&req.instrument)
            .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "no price yet"))?;
        (
            if matches!(side, PosSide::Long) {
                t.ask
            } else {
                t.bid
            },
            t.last,
        )
    };
    let id = with_account(&st, user, Origin::Client, |b| {
        b.open(
            user,
            req.instrument,
            side,
            req.qty,
            entry,
            fi.price_decimals,
            fi.qty_decimals,
            req.sl,
            req.tp,
        )
        .map_err(|_| err(StatusCode::PAYMENT_REQUIRED, "insufficient_margin"))
    })
    .await?;
    Ok(Json(DealDto {
        id,
        instrument: req.instrument,
        symbol: fi.symbol,
        side: req.side,
        qty: req.qty,
        entry,
        mark,
        pnl: 0,
        sl: req.sl,
        tp: req.tp,
        price_decimals: fi.price_decimals,
        qty_decimals: fi.qty_decimals,
    }))
}

/// Разместить лимитный (отложенный) ордер.
async fn place_pending(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<PendingReq>,
) -> Result<Json<PendingDto>, ApiErr> {
    // Лимитный ордер — такой же путь открытия позиции, только отложенный во времени.
    // Проверять только рыночный вход значило бы оставить дверь рядом с турникетом.
    let who = require_principal(&st, &headers).await?;
    ensure_can_open(&st, &who, req.instrument).await?;
    let user = who.user;
    let side = match req.side.as_str() {
        "buy" => PosSide::Long,
        "sell" => PosSide::Short,
        _ => return Err(err(StatusCode::BAD_REQUEST, "side must be buy|sell")),
    };
    if req.qty <= 0 || req.price <= 0 {
        return Err(err(StatusCode::BAD_REQUEST, "qty and price must be > 0"));
    }
    let fi = feed_by_id(&st, req.instrument)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown instrument"))?;
    let id = with_account(&st, user, Origin::Client, |b| {
        Ok(b.place_pending(
            user,
            req.instrument,
            side,
            req.qty,
            req.price,
            fi.price_decimals,
            fi.qty_decimals,
            req.sl,
            req.tp,
        ))
    })
    .await?;
    Ok(Json(PendingDto {
        id,
        instrument: req.instrument,
        symbol: fi.symbol,
        side: req.side,
        qty: req.qty,
        price: req.price,
        sl: req.sl,
        tp: req.tp,
        price_decimals: fi.price_decimals,
        qty_decimals: fi.qty_decimals,
    }))
}

async fn list_pending(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<PendingDto>>, ApiErr> {
    let user = require_user(&st, &headers).await?;
    let b = st.broker.lock().await;
    let out = b
        .pendings(user)
        .into_iter()
        .map(|p| PendingDto {
            id: p.id,
            instrument: p.instrument,
            symbol: feed_by_id(&st, p.instrument)
                .map(|f| f.symbol)
                .unwrap_or_default(),
            side: if matches!(p.side, PosSide::Long) {
                "buy"
            } else {
                "sell"
            }
            .to_string(),
            qty: p.qty,
            price: p.price,
            sl: p.sl,
            tp: p.tp,
            price_decimals: p.pd,
            qty_decimals: p.qd,
        })
        .collect();
    Ok(Json(out))
}

async fn cancel_pending(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Result<StatusCode, ApiErr> {
    let user = require_user(&st, &headers).await?;
    with_account(&st, user, Origin::Client, |b| {
        b.cancel_pending(user, id)
            .map_err(|_| err(StatusCode::NOT_FOUND, "unknown pending"))
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_closed(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ClosedDto>>, ApiErr> {
    let user = require_user(&st, &headers).await?;
    let b = st.broker.lock().await;
    let out = b
        .closed_deals(user)
        .into_iter()
        .rev()
        .map(|d| ClosedDto {
            instrument: d.instrument,
            symbol: feed_by_id(&st, d.instrument)
                .map(|f| f.symbol)
                .unwrap_or_default(),
            side: if matches!(d.side, PosSide::Long) {
                "buy"
            } else {
                "sell"
            }
            .to_string(),
            qty: d.qty,
            entry: d.entry,
            exit: d.exit,
            pnl: d.pnl,
            level: d.level,
            across_gap: d.across_gap,
            price_decimals: d.pd,
            qty_decimals: d.qd,
        })
        .collect();
    Ok(Json(out))
}

/// Список открытых позиций с live P&L.
async fn list_deals(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<DealDto>>, ApiErr> {
    let user = require_user(&st, &headers).await?;
    let m = marks(&st).await;
    let b = st.broker.lock().await;
    let out = b
        .positions(user)
        .into_iter()
        .map(|p| {
            let mark = *m.get(&p.instrument).unwrap_or(&p.entry);
            let symbol = feed_by_id(&st, p.instrument)
                .map(|f| f.symbol)
                .unwrap_or_default();
            DealDto {
                id: p.id,
                instrument: p.instrument,
                symbol,
                side: if matches!(p.side, PosSide::Long) {
                    "buy"
                } else {
                    "sell"
                }
                .to_string(),
                qty: p.qty,
                entry: p.entry,
                mark,
                pnl: p.unrealized(mark),
                sl: p.sl,
                tp: p.tp,
                price_decimals: p.pd,
                qty_decimals: p.qd,
            }
        })
        .collect();
    Ok(Json(out))
}

/// Закрыть позицию по текущей цене (last).
///
/// Allow-list здесь сознательно **не** проверяется: он ограничивает набор
/// разрешённого, а закрытие позицию сокращает. Если инструмент запретили, пока
/// позиция открыта, запереть человека в ней было бы хуже запрета — он не смог бы
/// выйти из того, что ему больше не разрешено держать. То же для отмены ордера.
async fn close_deal(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    let user = require_user(&st, &headers).await?;
    let m = marks(&st).await;
    let (pnl, mark) = with_account(&st, user, Origin::Client, |b| {
        let inst = b
            .positions(user)
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.instrument)
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown position"))?;
        let mark = *m.get(&inst).unwrap_or(&0);
        let pnl = b
            .close(user, id, mark)
            .map_err(|_| err(StatusCode::NOT_FOUND, "unknown position"))?;
        Ok((pnl, mark))
    })
    .await?;
    Ok(Json(serde_json::json!({ "pnl": pnl, "mark": mark })))
}

/// Сводка счёта: баланс/equity/маржа/free/open P&L (в центах).
async fn account(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AccountDto>, ApiErr> {
    let user = require_user(&st, &headers).await?;
    let m = marks(&st).await;
    let b = st.broker.lock().await;
    let balance = b.balance(user);
    let open_pnl = b.open_pnl(user, &m);
    let used = b.used_margin(user);
    let equity = balance + open_pnl;
    let model = match b.model(user) {
        broker::PositionModel::Netting => "netting",
        broker::PositionModel::Hedging => "hedging",
    };
    Ok(Json(AccountDto {
        model,
        balance,
        equity,
        used_margin: used,
        free_margin: equity - used,
        open_pnl,
    }))
}

/// Журнал счёта и итог сверки со слепком (TRM-13).
///
/// Отдаётся владельцу счёта: на вопрос «откуда у меня эти деньги» нужно отвечать
/// записями, а не уверениями. Экран администратора — не здесь, это `ADM-03`.
#[derive(Serialize)]
struct JournalEntryDto {
    seq: u64,
    at_ms: i64,
    /// `system` — движок, `client` — владелец счёта, `operator` — ручная проводка.
    source: &'static str,
    /// Основание и инцидент ручной проводки. У системных и клиентских событий пусты.
    ///
    /// Имени оператора здесь сознательно нет: клиенту нужно знать, что проводка
    /// ручная и по какому инциденту, а кто именно её сделал — вопрос разбора, и
    /// отвечать на него должен центр управления своей поверхностью, а не торговый API.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    incident: Option<String>,
    /// Каноническая строка события — та же, что записана и подписана.
    event: String,
}

#[derive(Serialize)]
struct JournalDto {
    /// `reconciled` · `no_entries` · `faulted` · `diverged`. Коды разные не из
    /// аккуратности: «записи нет» и «журнал испорчен» требуют разных действий.
    status: &'static str,
    /// Человекочитаемая причина, когда сверка не сошлась.
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    /// Баланс, восстановленный **из журнала** (а не взятый из слепка).
    replayed_balance: broker::Cents,
    balance: broker::Cents,
    entries: Vec<JournalEntryDto>,
}

async fn account_journal(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<JournalDto>, ApiErr> {
    let user = require_user(&st, &headers).await?;
    let (snap, lev, start_balance) = {
        let b = st.broker.lock().await;
        (b.snapshot(user), b.leverage(), b.start_balance())
    };
    let js = st.journals.lock().await;
    let empty = Journal::new(lev, start_balance);
    let j = js.get(&user).unwrap_or(&empty);
    let status = journal::audit(j, &snap);
    let entries = j
        .events()
        .iter()
        .map(|e| {
            let op = e.origin().operator();
            JournalEntryDto {
                seq: e.seq(),
                at_ms: e.at_ms(),
                source: e.origin().tag(),
                reason: op.map(|o| o.reason().to_string()),
                incident: op.map(|o| o.incident().to_string()),
                event: e.kind().encode(),
            }
        })
        .collect();
    Ok(Json(JournalDto {
        status: status.tag(),
        detail: if status.ok() {
            None
        } else {
            Some(status.to_string())
        },
        replayed_balance: j.replay().balance,
        balance: snap.balance,
        entries,
    }))
}

// ---- SSO: обмен handoff-JWT на сессию терминала (ADR-023, Т2) --------------

#[derive(Deserialize)]
struct SessionReq {
    /// handoff-JWT платформы, полученный фронтом через postMessage.
    token: String,
    /// Сайт из `?site=` терминала — должен совпасть с `tenant` токена.
    site: String,
}
#[derive(Serialize)]
struct SessionResp {
    token: String,
    expires_in: u64,
}

/// Обменять валидный handoff-JWT на сессию терминала. Счёт — по `(tenant, sub)`.
async fn session_create(
    State(st): State<AppState>,
    Json(req): Json<SessionReq>,
) -> Result<Json<SessionResp>, ApiErr> {
    let claims = st
        .sso
        .validate(&req.token, &req.site)
        .await
        .map_err(sso_err)?;
    let user = st
        .store
        .resolve_identity(&claims.tenant, &claims.sub)
        .await
        .map_err(|_| {
            err(
                StatusCode::SERVICE_UNAVAILABLE,
                "identity store unavailable",
            )
        })?;
    let expires_in = st
        .sso
        .mint(claims.jti.clone(), user, claims.tenant.clone())
        .await;
    Ok(Json(SessionResp {
        token: claims.jti,
        expires_in,
    }))
}

/// Погасить сессию терминала (логаут из кабинета).
async fn session_logout(State(st): State<AppState>, headers: HeaderMap) -> StatusCode {
    if let Some(t) = bearer(&headers) {
        st.sso.logout(t).await;
    }
    StatusCode::NO_CONTENT
}

fn sso_err(e: sso::SsoError) -> ApiErr {
    let code = match e {
        sso::SsoError::TenantMismatch => StatusCode::FORBIDDEN,
        sso::SsoError::NoKeys => StatusCode::SERVICE_UNAVAILABLE,
        sso::SsoError::Disabled => StatusCode::NOT_IMPLEMENTED,
        sso::SsoError::BadToken | sso::SsoError::Replay => StatusCode::UNAUTHORIZED,
    };
    err(code, e.as_str())
}

/// Фоновый монитор: триггеры лимитных ордеров и SL/TP по текущим ценам (ADR-016).
pub fn spawn_monitor(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
        loop {
            tick.tick().await;
            monitor_tick(&st).await;
        }
    });
}

/// Один проход монитора.
///
/// Вынесен из цикла сознательно: путь триггеров проверяется тестами **тем же кодом**,
/// которым он работает в бою, а не ожиданием таймера. Сработавший стоп меняет счёт так
/// же, как ручная операция, и обязан оставлять такую же запись — спорят как раз о нём.
pub async fn monitor_tick(st: &AppState) {
    let m = marks(st).await;
    if m.is_empty() {
        return;
    }
    // Разрыв ряда определяет тот, кто принимает цены (Р-073, TRM-19).
    // Молчание дольше порога взводит признак; первый же прогон по
    // свежим ценам его СНИМАЕТ и помечает свои записи — дальше ряд
    // снова непрерывен. Порог тот же, что у `/health`: два разных
    // числа для одного вопроса однажды разойдутся.
    let across_gap = st.note_feed_gap();
    let changed = { st.broker.lock().await.check(&m, across_gap) };
    for user in changed {
        let lock = account_lock(st, user).await;
        let _guard = lock.lock().await;
        let (snap, kinds, lev, start_balance) = {
            let mut b = st.broker.lock().await;
            (
                b.snapshot(user),
                b.take_events(user),
                b.leverage(),
                b.start_balance(),
            )
        };
        // Триггеры SL/TP и лимитных ордеров меняют счёт так же, как ручные
        // операции, поэтому и проверяются так же, и в журнал идут так же.
        // Иначе защита стояла бы только на том пути, о котором мы помним, а
        // сработавший стоп не оставлял бы следа — при том что спорят именно о нём.
        let violations = broker::check_snapshot(&snap, lev);
        if !violations.is_empty() {
            st.quarantined.lock().await.insert(user);
            for v in &violations {
                eprintln!(
                    "[monitor] ИНВАРИАНТ НАРУШЕН триггером, счёт {} в карантине: {v}",
                    user.0
                );
            }
            continue; // испорченное не записываем и не чиним
        }
        let (sealed, before_bus) = {
            let mut js = st.journals.lock().await;
            let j = js
                .entry(user)
                .or_insert_with(|| Journal::new(lev, start_balance));
            // «До» — по свёртке журнала (TRM-40). Слепка «до» здесь нет и быть не
            // может: монитор узнаёт о затронутых счетах уже после мутации.
            let before_bus = bus::Before {
                closed: j.state().closed.len(),
                balance: j.state().balance,
            };
            let sealed = j.seal(now_ms(), &Origin::System, &kinds);
            let diverged = journal::reconcile(&j.preview(&sealed), &snap);
            if !diverged.is_empty() {
                drop(js);
                st.quarantined.lock().await.insert(user);
                for d in &diverged {
                    eprintln!(
                        "[monitor] ЖУРНАЛ НЕ СХОДИТСЯ после триггера, счёт {} в карантине: {d}",
                        user.0
                    );
                }
                continue; // цену уже не отыграть назад, но и чинить молча нельзя
            }
            (sealed, before_bus)
        };
        // Сработавший стоп — такая же сделка, как нажатая кнопка, и наружу обязан
        // уходить так же. Путь, на котором событие собирается только у ручных
        // операций, молчит ровно про то, о чём спорят с клиентом.
        let outgoing = bus_envelopes(st, user, before_bus, &snap, &kinds, &sealed).await;
        if let Err(e) = st
            .store
            .save_account(user, &snap, &sealed, &outgoing)
            .await
        {
            eprintln!("[monitor] не удалось сохранить счёт {}: {e}", user.0);
            continue; // в память журнал не принимаем: в хранилище его нет
        }
        commit_journal(st, user, sealed).await;
    }
}

async fn ws_stream(State(st): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, st))
}
async fn handle_ws(mut socket: WebSocket, st: AppState) {
    let mut rx = st.events_tx.subscribe();
    loop {
        match rx.recv().await {
            Ok(msg) => {
                if socket.send(Message::Text(msg)).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

// ======================= Тесты allow-list инструментов ======================
//
// Регуляторное требование (ADR-023 Т3): сайту нельзя торговать тем, что его
// юрисдикции не разрешено. Тесты идут **через роутер**, а не по `ensure_can_open`
// напрямую: проверка, вызванная из одного хендлера и забытая в другом, — это
// именно тот дефект, который надо ловить (урок BUG-013 — тест должен проверять
// барьер на его месте, а не свойство функции в вакууме).
#[cfg(test)]
mod allow_list_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const SITE: &str = "apex-ru";

    /// Роутер в тенантном режиме с готовой сессией сайта `apex-ru`.
    async fn tenant_app(source: TenantSource) -> (Router, String) {
        let mut st = build_state();
        st.sso = Arc::new(sso::Sso::new("http://jwks.invalid/jwks".into()));
        st.sso_enabled = true;
        st.tenant_instruments = Arc::new(source);
        let token = "session-token".to_string();
        st.sso
            .mint(token.clone(), DEFAULT_USER, SITE.to_string())
            .await;
        (router(st), token)
    }

    /// Источник, выдающий сайту перечисленные **символы** — ровно в той форме, в
    /// какой их присылает CMS. Перевод в id идёт через ту же границу, что в бою.
    fn allow(site: &str, symbols: &[&str]) -> TenantSource {
        let index = symbol_index(&feed::instruments());
        let list = TenantAllowList::resolve(
            &index,
            &symbols.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        );
        let mut m = HashMap::new();
        m.insert(site.to_string(), list);
        TenantSource::Static(m)
    }

    async fn post(
        app: &Router,
        uri: &str,
        token: Option<&str>,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let mut b = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        let req = b
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        read(app.clone().oneshot(req).await.unwrap()).await
    }

    async fn get(app: &Router, uri: &str, token: Option<&str>) -> (StatusCode, serde_json::Value) {
        let mut b = Request::builder().method("GET").uri(uri);
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        read(
            app.clone()
                .oneshot(b.body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await
    }

    async fn read(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
        };
        (status, v)
    }

    fn deal(instrument: u32) -> serde_json::Value {
        serde_json::json!({"instrument": instrument, "side": "buy", "qty": 100})
    }
    fn pending(instrument: u32) -> serde_json::Value {
        serde_json::json!({"instrument": instrument, "side": "buy", "qty": 100, "price": 1000})
    }

    // ---- Разбор источника ---------------------------------------------------

    fn index() -> HashMap<String, u32> {
        symbol_index(&feed::instruments())
    }

    #[test]
    fn parses_source() {
        let m =
            parse_tenant_instruments(Some("apex-ru:BTCUSD,ETHUSD,BNBUSD;other:XRPUSD"), &index())
                .expect("источник задан");
        assert_eq!(
            m.get("apex-ru"),
            Some(&TenantAllowList::Resolved(vec![1, 2, 3]))
        );
        assert_eq!(m.get("other"), Some(&TenantAllowList::Resolved(vec![5])));
    }

    /// Пустой список у сайта — валидное значение «сайту не разрешено ничего»,
    /// а не «фильтра нет». Разница регуляторная, поэтому проверяем отдельно.
    #[test]
    fn empty_list_is_a_value_not_absence() {
        let m = parse_tenant_instruments(Some("apex-ru:"), &index()).expect("источник задан");
        assert_eq!(m.get("apex-ru"), Some(&TenantAllowList::Resolved(vec![])));
    }

    #[test]
    fn absent_source_is_none() {
        assert!(parse_tenant_instruments(None, &index()).is_none());
        assert!(parse_tenant_instruments(Some("   "), &index()).is_none());
    }

    // ---- Р-024: контракт — символы, перевод в id стоит у нас ----------------

    /// Символы, которыми инструмент называют снаружи, ведут к одному и тому же
    /// внутреннему id: канонический платформенный (`BTCUSD`, так хранит CMS и
    /// отдаёт MDS), биржевой (`BTCUSDT`) и показанный на витрине (`BTC-USDT`).
    #[test]
    fn symbol_maps_to_internal_id() {
        let idx = index();
        for s in ["BTCUSD", "BTCUSDT", "BTC-USDT", "btcusd", "btc-usdt"] {
            assert_eq!(
                idx.get(&normalize_symbol(s)),
                Some(&1),
                "символ {s} обязан указывать на инструмент 1"
            );
        }
        assert_eq!(idx.get(&normalize_symbol("MANAUSD")), Some(&30));
    }

    /// Нормализация снимает только оформление. Если она начнёт склеивать разные
    /// имена, два инструмента отзовутся на один символ — и разрешения поплывут
    /// молча. Этот тест и есть доказательство, что такого не происходит.
    #[test]
    fn symbol_index_has_no_collisions() {
        let instruments = feed::instruments();
        let idx = symbol_index(&instruments);
        // У каждого инструмента ровно два различимых написания: XXXUSDT и XXXUSD
        // (`XXX-USDT` после нормализации совпадает с первым).
        assert_eq!(
            idx.len(),
            instruments.len() * 2,
            "каждый символ обязан указывать ровно на один инструмент"
        );
        for f in &instruments {
            assert_eq!(idx.get(&normalize_symbol(&f.binance)), Some(&f.id));
            assert_eq!(idx.get(&normalize_symbol(&f.symbol)), Some(&f.id));
        }
    }

    /// **Главный пункт Р-024.** Неизвестный символ — отказ, а не пропуск.
    ///
    /// Молча выбросить его значило бы тихо сузить права: разрешённый инструмент
    /// исчез бы из списка, и никто бы об этом не узнал. Поэтому не принимается
    /// **весь** список сайта: частично сопоставленный список — это уже не тот
    /// список, который прислал источник.
    #[test]
    fn unknown_symbol_rejects_the_whole_list() {
        let list = TenantAllowList::resolve(
            &index(),
            &["BTCUSD".into(), "XAUUSD".into(), "ETHUSD".into()],
        );
        match list {
            TenantAllowList::Unknown(unknown) => assert_eq!(unknown, vec!["XAUUSD".to_string()]),
            TenantAllowList::Resolved(ids) => {
                panic!(
                    "неизвестный символ не должен молча исчезать, а список — сужаться до {ids:?}"
                )
            }
        }
    }

    /// Нераспознанный символ закрывает торговлю, и причина отличается от
    /// «источника нет»: чинится правкой списка в CMS, а не связью с ней.
    #[tokio::test]
    async fn unknown_symbol_closes_trading_with_its_own_reason() {
        let (app, tok) = tenant_app(allow(SITE, &["BTCUSD", "XAUUSD"])).await;
        let (status, b) = post(&app, "/deals", Some(&tok), deal(1)).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "список с неизвестным символом не принимается целиком"
        );
        assert_eq!(
            b["error"], "tenant_instruments_unreadable",
            "причина обязана отличаться от «источника нет»"
        );
    }

    // ---- Граница: открытие позиции -----------------------------------------

    /// Рыночный вход по инструменту вне списка сайта — 403.
    #[tokio::test]
    async fn open_deal_rejects_instrument_outside_allow_list() {
        let (app, tok) = tenant_app(allow(SITE, &["BTCUSD"])).await;
        let (status, body) = post(&app, "/deals", Some(&tok), deal(2)).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "инструмент вне allow-list не должен торговаться"
        );
        assert_eq!(body["error"], "instrument_not_allowed");
    }

    /// Лимитный ордер — тот же путь открытия позиции. Дверь рядом с турникетом.
    #[tokio::test]
    async fn pending_rejects_instrument_outside_allow_list() {
        let (app, tok) = tenant_app(allow(SITE, &["BTCUSD"])).await;
        let (status, body) = post(&app, "/pending", Some(&tok), pending(2)).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "отложенный ордер обязан проверяться так же, как рыночный"
        );
        assert_eq!(body["error"], "instrument_not_allowed");
    }

    /// Разрешённый инструмент проходит границу (лимитный ордер не требует живой цены).
    #[tokio::test]
    async fn allowed_instrument_passes() {
        let (app, tok) = tenant_app(allow(SITE, &["BTCUSD"])).await;
        let (status, _) = post(&app, "/pending", Some(&tok), pending(1)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "разрешённый инструмент обязан торговаться"
        );
    }

    /// Пустой список запрещает всё — включая инструмент №1.
    #[tokio::test]
    async fn empty_allow_list_denies_everything() {
        let (app, tok) = tenant_app(allow(SITE, &[])).await;
        let (status, _) = post(&app, "/deals", Some(&tok), deal(1)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    // ---- Fail-closed --------------------------------------------------------

    /// Источник не сконфигурирован → торговля закрыта (КОНТРАКТЫ §4б).
    /// Отказ инфраструктуры не должен превращаться в разрешение.
    #[tokio::test]
    async fn missing_source_closes_trading() {
        let (app, tok) = tenant_app(TenantSource::None).await;
        for (uri, body) in [("/deals", deal(1)), ("/pending", pending(1))] {
            let (status, b) = post(&app, uri, Some(&tok), body).await;
            assert_eq!(
                status,
                StatusCode::SERVICE_UNAVAILABLE,
                "{uri}: без конфига торговли быть не должно"
            );
            assert_eq!(
                b["error"], "tenant_instruments_unavailable",
                "{uri}: причина обязана отличаться от «не разрешено»"
            );
        }
    }

    /// Сайта нет в реестре → тоже закрыто, и причина та же: списка нет.
    #[tokio::test]
    async fn unknown_site_closes_trading() {
        let (app, tok) = tenant_app(allow("some-other-site", &["BTCUSD", "ETHUSD"])).await;
        let (status, b) = post(&app, "/deals", Some(&tok), deal(1)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(b["error"], "tenant_instruments_unavailable");
    }

    // ---- Что allow-list ограничивать НЕ должен -----------------------------

    /// Закрытие позиции и отмена ордера не ограничиваются: список ограничивает
    /// набор разрешённого, а закрытие сокращает позицию. Иначе человека запирают
    /// в том, что ему больше не разрешено держать. Признак прохождения — 404
    /// (позиции нет), а не 403.
    #[tokio::test]
    async fn closing_is_never_blocked_by_allow_list() {
        let (app, tok) = tenant_app(allow(SITE, &[])).await;
        let (status, _) = post(&app, "/deals/1/close", Some(&tok), serde_json::Value::Null).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "закрытие не должно упираться в allow-list"
        );
    }

    /// Отмена отложенного ордера — тоже сокращение риска, тоже не ограничивается.
    /// Пара к предыдущему тесту: если «для единообразия» границу поставят на все
    /// торговые ручки подряд, покраснеет один из этих двух.
    #[tokio::test]
    async fn cancelling_is_never_blocked_by_allow_list() {
        let (app, tok) = tenant_app(allow(SITE, &[])).await;
        let req = Request::builder()
            .method("DELETE")
            .uri("/pending/1")
            .header("authorization", format!("Bearer {tok}"))
            .body(Body::empty())
            .unwrap();
        let (status, _) = read(app.oneshot(req).await.unwrap()).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "отмена ордера не должна упираться в allow-list"
        );
    }

    /// SL/TP — атрибуты позиции (`Position { sl, tp }`), а не отдельные ордера:
    /// закрытие по ним идёт фоновым монитором, мимо роутера и мимо границы. Тест
    /// фиксирует именно это: позиция с SL/TP на разрешённом инструменте заводится
    /// одним запросом, отдельной «ручки закрытия» для неё не требуется.
    #[tokio::test]
    async fn sl_tp_travel_with_the_position_not_as_orders() {
        let (app, tok) = tenant_app(allow(SITE, &["BTCUSD"])).await;
        let body = serde_json::json!({"instrument": 1, "side": "buy", "qty": 100, "price": 1000, "sl": 900, "tp": 1200});
        let (status, v) = post(&app, "/pending", Some(&tok), body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["sl"], 900);
        assert_eq!(v["tp"], 1200);
    }

    /// Чтение позиций и счёта сайтом не ограничивается.
    #[tokio::test]
    async fn reading_account_is_not_blocked() {
        let (app, tok) = tenant_app(allow(SITE, &[])).await;
        let (status, _) = get(&app, "/account", Some(&tok)).await;
        assert_eq!(status, StatusCode::OK);
    }

    // ---- Витрина ------------------------------------------------------------

    #[tokio::test]
    async fn instruments_listing_filtered_by_site() {
        let (app, _) = tenant_app(allow(SITE, &["ETHUSD", "BNBUSD"])).await;
        let (status, body) = get(&app, "/instruments?site=apex-ru", None).await;
        assert_eq!(status, StatusCode::OK);
        let ids: Vec<u64> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_u64().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![2, 3],
            "витрина обязана показывать только разрешённое"
        );
    }

    /// Без конфига витрина отказывает кодом, а не отдаёт пустой список: пустой
    /// список читается как «сайту нечем торговать» (BUG-011).
    #[tokio::test]
    async fn instruments_listing_fails_loudly_without_config() {
        let (app, _) = tenant_app(TenantSource::None).await;
        let (status, b) = get(&app, "/instruments?site=apex-ru", None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(b["error"], "tenant_instruments_unavailable");
    }

    /// Сайту не разрешено ничего — витрина говорит это кодом, а не пустым массивом.
    /// Пустой список в ответе неотличим от «конфиг не доехал» (§4б), а молчаливое
    /// исчезновение витрины запрещено отдельно (Р-025).
    #[tokio::test]
    async fn empty_allow_list_is_said_out_loud_not_rendered_as_empty() {
        let (app, _) = tenant_app(allow(SITE, &[])).await;
        let (status, b) = get(&app, "/instruments?site=apex-ru", None).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "пустой список инструментов не отдаётся никогда"
        );
        assert_eq!(b["error"], "no_instruments_allowed");
    }

    /// Инструмент без котировки не отдаёт «0%»: изменение неизвестно.
    ///
    /// Самый дорогой класс у нас — отказ, надевший правдоподобное значение.
    /// Здесь он стоял в поле, которое витрина красит зелёным: «+0.00%»
    /// читается как «рынок стоял», тогда как известно только «тикера нет».
    /// Четыре соседних поля той же строки врать не умели никогда.
    #[tokio::test]
    async fn change_is_unknown_not_zero_when_there_is_no_ticker() {
        let (app, _) = tenant_app(allow(SITE, &["BTCUSD"])).await;
        let (status, body) = get(&app, "/instruments?site=apex-ru", None).await;
        assert_eq!(status, StatusCode::OK);

        let row = &body.as_array().expect("список")[0];
        assert!(row["last"].is_null(), "предпосылка теста: тикера нет");
        assert!(
            row["change"].is_null(),
            "изменение без котировки обязано быть null, а не {}",
            row["change"]
        );
    }

    /// Без `?site=` витрина отдаёт всё: это standalone-режим, юрисдикции нет.
    #[tokio::test]
    async fn instruments_listing_unfiltered_without_site() {
        let (app, _) = tenant_app(allow(SITE, &["BTCUSD"])).await;
        let (status, body) = get(&app, "/instruments", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_array().unwrap().len(), feed::instruments().len());
    }

    // ---- Standalone ---------------------------------------------------------

    /// Без SSO сайта нет, ограничивать нечем и не от чьего имени: торгуем всем.
    #[tokio::test]
    async fn standalone_is_not_restricted() {
        let app = router(build_state()); // sso выключен: tenant отсутствует
        let (status, _) = post(&app, "/pending", None, pending(30)).await;
        assert_eq!(status, StatusCode::OK);
    }
}

// ============ Проводка инвариантов: отказ до записи и карантин ==============
//
// Тесты идут ЧЕРЕЗ РОУТЕР, а не по `check_snapshot`: проверка, которую перестали
// звать, остаётся зелёной в юнит-тестах самой проверки (урок BUG-013).
#[cfg(test)]
mod invariant_wiring_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// Слепок с нарушенным инвариантом: позиция с нулевым объёмом.
    fn corrupt_snapshot() -> broker::AccountSnapshot {
        broker::AccountSnapshot {
            balance: 1_000_000,
            next_id: 1,
            positions: vec![broker::Position {
                id: 1,
                instrument: 1,
                side: PosSide::Long,
                qty: 0,
                entry: 5000,
                pd: 2,
                qd: 3,
                margin: 5_000,
                sl: None,
                tp: None,
            }],
            pendings: vec![],
            closed: vec![],
            model: broker::PositionModel::Hedging,
        }
    }

    /// Хранилище, отдающее на старте счёт с нарушенным инвариантом.
    struct CorruptStore;
    #[async_trait::async_trait]
    impl persistence::Persistence for CorruptStore {
        async fn init(&self) -> Result<(), persistence::StoreError> {
            Ok(())
        }
        async fn load_all(&self) -> Result<persistence::LoadedState, persistence::StoreError> {
            Ok(persistence::LoadedState {
                accounts: vec![(DEFAULT_USER, corrupt_snapshot())],
                journals: vec![],
            })
        }
        async fn save_account(
            &self,
            _u: UserId,
            _s: &broker::AccountSnapshot,
            _e: &[journal::AccountEvent],
            _out: &[persistence::OutboxRecord],
        ) -> Result<(), persistence::StoreError> {
            Ok(())
        }
    }

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let b = Request::builder().method(method).uri(uri);
        let req = match body {
            Some(v) => b
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&v).unwrap()))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
        };
        (status, v)
    }

    fn pending_body() -> serde_json::Value {
        serde_json::json!({"instrument": 1, "side": "buy", "qty": 100, "price": 1000})
    }

    // ---- Отказ ДО записи ----------------------------------------------------

    /// Мутация поверх неверного состояния отклоняется, изменение не фиксируется.
    /// Счёт при этом НЕ в карантине — карантин ставится только при чтении из
    /// хранилища; здесь проверяется именно проверка перед записью.
    #[tokio::test]
    async fn mutation_over_broken_state_is_rejected_before_write() {
        let st = build_state();
        st.broker
            .lock()
            .await
            .restore(DEFAULT_USER, corrupt_snapshot());
        let app = router(st.clone());

        let (status, body) = send(&app, "POST", "/pending", Some(pending_body())).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "операция, оставляющая счёт неверным, обязана быть отклонена"
        );
        assert_eq!(
            body["error"], "invariant_violated",
            "отказ обязан отличаться от делового"
        );

        // Изменение не зафиксировано: ордер не добавился.
        let (_, list) = send(&app, "GET", "/pending", None).await;
        assert_eq!(
            list.as_array().map(|a| a.len()),
            Some(0),
            "изменение не должно фиксироваться"
        );
    }

    /// Отказ по инварианту отличим от делового отказа. Их нельзя путать: первый
    /// означает «состояние сломано», второй — «денег не хватило».
    #[tokio::test]
    async fn invariant_rejection_differs_from_business_rejection() {
        let st = build_state();
        st.broker
            .lock()
            .await
            .restore(DEFAULT_USER, corrupt_snapshot());
        let app = router(st);
        let (status, body) = send(&app, "POST", "/pending", Some(pending_body())).await;
        assert_ne!(status, StatusCode::PAYMENT_REQUIRED);
        assert_ne!(body["error"], "insufficient_margin");
    }

    // ---- Карантин при чтении ------------------------------------------------

    /// Сервис СТАРТУЕТ на испорченном состоянии — стоп сервиса не применяется
    /// нигде: одна испорченная строка не должна лишать обслуживания всех.
    #[tokio::test]
    async fn broken_state_quarantines_account_but_service_starts() {
        let st = build_state_with(Arc::new(CorruptStore));
        restore_state(&st)
            .await
            .expect("сервис обязан стартовать, а не падать");
        assert!(
            st.quarantined.lock().await.contains(&DEFAULT_USER),
            "счёт обязан уйти в карантин"
        );
    }

    /// В карантине: чтение разрешено, мутации отклоняются.
    #[tokio::test]
    async fn quarantined_account_reads_but_does_not_mutate() {
        let st = build_state_with(Arc::new(CorruptStore));
        restore_state(&st).await.unwrap();
        let app = router(st);

        let (read_status, _) = send(&app, "GET", "/account", None).await;
        assert_eq!(
            read_status,
            StatusCode::OK,
            "чтение в карантине обязано работать"
        );

        let (write_status, body) = send(&app, "POST", "/pending", Some(pending_body())).await;
        assert_eq!(
            write_status,
            StatusCode::LOCKED,
            "мутации в карантине обязаны отклоняться"
        );
        assert_eq!(body["error"], "account_quarantined");
    }

    /// Испорченное состояние сохраняется КАК ЕСТЬ: молчаливая починка уничтожила
    /// бы улику и скрыла причину (КОНТРАКТЫ §4в).
    #[tokio::test]
    async fn broken_state_is_not_silently_repaired() {
        let st = build_state_with(Arc::new(CorruptStore));
        restore_state(&st).await.unwrap();
        let snap = st.broker.lock().await.snapshot(DEFAULT_USER);
        assert_eq!(snap.positions.len(), 1, "позиция не должна исчезнуть");
        assert_eq!(
            snap.positions[0].qty, 0,
            "нулевой объём обязан сохраниться как улика"
        );
        assert!(
            !broker::check_snapshot(&snap, 1).is_empty(),
            "состояние обязано остаться помеченным как неверное"
        );
    }

    /// Здоровое состояние в карантин не уходит — проверка не срабатывает вхолостую.
    #[tokio::test]
    async fn healthy_account_is_not_quarantined() {
        let st = build_state();
        restore_state(&st).await.unwrap();
        assert!(st.quarantined.lock().await.is_empty());
        let app = router(st);
        let (status, _) = send(&app, "POST", "/pending", Some(pending_body())).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "нормальная торговля не должна страдать от проверок"
        );
    }
}

// ============ Проводка журнала: от запроса до восстановленного счёта =========
//
// Тесты идут ЧЕРЕЗ РОУТЕР и через настоящий монитор, а не по `Journal` напрямую:
// журнал, в который перестали писать, остаётся зелёным в собственных юнит-тестах.
// Класс «написано и не вызывается» модульными тестами не ловится в принципе — это
// уже стоило нам пустого инварианта (TRM-06).
#[cfg(test)]
mod journal_wiring_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let b = Request::builder().method(method).uri(uri);
        let req = match body {
            Some(v) => b
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&v).unwrap()))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
        };
        (status, v)
    }

    /// Цена нужна торговым ручкам: без тикера открытие отвечает «нет цены».
    async fn with_price(st: &AppState, instrument: u32, px: i64) {
        st.tickers.lock().await.insert(
            instrument,
            feed::Ticker {
                last: px,
                change: 0.0,
                high: px,
                bid: px,
                ask: px,
            },
        );
    }

    fn deal_body() -> serde_json::Value {
        serde_json::json!({"instrument": 1, "side": "buy", "qty": 1000})
    }

    async fn events_of(st: &AppState) -> Vec<String> {
        st.journals
            .lock()
            .await
            .get(&DEFAULT_USER)
            .map(|j| j.events().iter().map(|e| e.kind().encode()).collect())
            .unwrap_or_default()
    }

    // ---- Сделка из терминала оставляет след ---------------------------------

    /// Открытие через HTTP обязано появиться в журнале, а остаток — выводиться из него.
    #[tokio::test]
    async fn opening_a_deal_through_the_router_lands_in_the_journal() {
        let st = build_state();
        with_price(&st, 1, 50_000).await;
        let app = router(st.clone());

        let (status, _) = send(&app, "POST", "/deals", Some(deal_body())).await;
        assert_eq!(status, StatusCode::OK);

        let kinds = events_of(&st).await;
        assert!(
            kinds.iter().any(|k| k.starts_with("account_opened")),
            "заведение счёта обязано быть записано: {kinds:?}"
        );
        assert!(
            kinds.iter().any(|k| k.starts_with("position_opened")),
            "сделка обязана быть записана: {kinds:?}"
        );

        let (status, body) = send(&app, "GET", "/account/journal", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["status"], "reconciled",
            "журнал обязан сходиться со слепком: {body}"
        );
        assert_eq!(
            body["replayed_balance"], body["balance"],
            "остаток обязан выводиться из журнала"
        );
        assert_eq!(body["entries"].as_array().unwrap().len(), 2);
        assert_eq!(
            body["entries"][1]["source"], "client",
            "сделку сделал человек, и это видно"
        );
    }

    /// Закрытие записывается с причиной «по команде» — иначе срабатывание стопа и
    /// действие человека в истории неразличимы.
    #[tokio::test]
    async fn closing_through_the_router_is_recorded_with_its_cause() {
        let st = build_state();
        with_price(&st, 1, 50_000).await;
        let app = router(st.clone());
        let (_, deal) = send(&app, "POST", "/deals", Some(deal_body())).await;
        let id = deal["id"].as_u64().unwrap();

        let (status, _) = send(&app, "POST", &format!("/deals/{id}/close"), None).await;
        assert_eq!(status, StatusCode::OK);

        let kinds = events_of(&st).await;
        assert!(
            kinds
                .iter()
                .any(|k| k.contains("position_closed") && k.contains("cause=command")),
            "{kinds:?}"
        );
        let (_, body) = send(&app, "GET", "/account/journal", None).await;
        assert_eq!(body["status"], "reconciled");
    }

    /// Отложенный ордер и его отмена — тоже изменения состояния.
    #[tokio::test]
    async fn pending_order_and_its_cancellation_are_recorded() {
        let st = build_state();
        let app = router(st.clone());
        let body = serde_json::json!({"instrument": 1, "side": "buy", "qty": 100, "price": 1000});
        let (_, p) = send(&app, "POST", "/pending", Some(body)).await;
        let id = p["id"].as_u64().unwrap();
        send(&app, "DELETE", &format!("/pending/{id}"), None).await;

        let kinds = events_of(&st).await;
        assert!(
            kinds.iter().any(|k| k.starts_with("pending_placed")),
            "{kinds:?}"
        );
        assert!(
            kinds.iter().any(|k| k.starts_with("pending_cancelled")),
            "{kinds:?}"
        );
    }

    /// Сработавший стоп проходит через монитор, а не через запрос — и обязан
    /// записываться тем же механизмом, иначе журнал пропускает ровно те события,
    /// о которых спорят чаще всего.
    /// Закрытая сделка отдаётся НАРУЖУ с уровнем и признаком разрыва (TRM-38).
    ///
    /// Проверяется именно ответ ручки, а не внутренняя запись: витрина читает
    /// его, и до этой правки получала одну цену исполнения. Журнал обе
    /// величины нёс и раньше — но журнал читает владелец счёта, а проекции
    /// читают другие, и одна величина, записанная в одном месте, не
    /// становится доступной во всех.
    #[tokio::test]
    async fn closed_deal_is_served_with_level_and_gap() {
        let st = build_state();
        with_price(&st, 1, 50_000).await;
        let app = router(st.clone());
        let body = serde_json::json!({"instrument": 1, "side": "buy", "qty": 1000, "sl": 49_000});
        let (status, _) = send(&app, "POST", "/deals", Some(body)).await;
        assert_eq!(status, StatusCode::OK);

        // Рынок перепрыгнул стоп: исполнение пойдёт по 48 000, а ставили 49 000.
        with_price(&st, 1, 48_000).await;
        monitor_tick(&st).await;

        let (status, v) = send(&app, "GET", "/deals/closed", None).await;
        assert_eq!(status, StatusCode::OK);
        let d = &v.as_array().expect("список сделок")[0];

        assert_eq!(d["exit"], 48_000, "цена исполнения");
        assert_eq!(
            d["level"], 49_000,
            "и поставленный уровень — без него проскальзывание не посчитать"
        );
        assert!(
            !d["across_gap"].is_null(),
            "признак разрыва обязан присутствовать: отсутствие означает «неизвестно», а здесь известно"
        );
    }

    #[tokio::test]
    async fn stop_loss_fired_by_the_monitor_is_recorded_as_a_system_event() {
        let st = build_state();
        with_price(&st, 1, 50_000).await;
        let app = router(st.clone());
        let body = serde_json::json!({"instrument": 1, "side": "buy", "qty": 1000, "sl": 49_000});
        let (status, _) = send(&app, "POST", "/deals", Some(body)).await;
        assert_eq!(status, StatusCode::OK);

        with_price(&st, 1, 48_000).await;
        monitor_tick(&st).await;

        let kinds = events_of(&st).await;
        assert!(
            kinds
                .iter()
                .any(|k| k.contains("position_closed") && k.contains("cause=stop_loss")),
            "срабатывание стопа обязано попасть в журнал: {kinds:?}"
        );
        let last = st
            .journals
            .lock()
            .await
            .get(&DEFAULT_USER)
            .unwrap()
            .events()
            .last()
            .unwrap()
            .clone();
        assert_eq!(
            last.origin(),
            &Origin::System,
            "у триггера нет человека-автора"
        );

        let (_, jb) = send(&app, "GET", "/account/journal", None).await;
        assert_eq!(
            jb["status"], "reconciled",
            "после срабатывания стопа остаток обязан сходиться: {jb}"
        );
        assert!(
            st.quarantined.lock().await.is_empty(),
            "нормальное срабатывание — не повод для карантина"
        );
    }

    // ---- Хранилище: журнал переживает перезапуск ----------------------------

    /// Журнал и слепок пишутся в хранилище вместе и поднимаются вместе. Проверяем
    /// на настоящем пути записи, а не на заглушке: заглушка, которая ничего не делает,
    /// однажды уже «проверяла» у нас пустой инвариант.
    #[tokio::test]
    async fn journal_survives_a_restart_through_the_store() {
        let store = Arc::new(persistence::MemoryStore::new());
        let before = {
            let st = build_state_with(store.clone());
            with_price(&st, 1, 50_000).await;
            let app = router(st.clone());
            send(&app, "POST", "/deals", Some(deal_body())).await;
            send(
                &app,
                "POST",
                "/pending",
                Some(
                    serde_json::json!({"instrument": 2, "side": "sell", "qty": 10, "price": 9_000}),
                ),
            )
            .await;
            let (_, acct) = send(&app, "GET", "/account", None).await;
            acct["balance"].clone()
        };

        // Новый процесс: то же хранилище, пустая память.
        let st2 = build_state_with(store.clone());
        restore_state(&st2).await.expect("подъём обязан пройти");
        let app2 = router(st2.clone());

        assert!(
            st2.quarantined.lock().await.is_empty(),
            "сошедшийся счёт в карантин не уходит"
        );
        let (_, jb) = send(&app2, "GET", "/account/journal", None).await;
        assert_eq!(
            jb["status"], "reconciled",
            "после рестарта журнал обязан сходиться: {jb}"
        );
        assert_eq!(jb["balance"], before, "остаток обязан пережить рестарт");
        assert_eq!(
            jb["replayed_balance"], before,
            "и обязан выводиться из журнала"
        );
        assert!(
            jb["entries"].as_array().unwrap().len() >= 3,
            "лента обязана подняться целиком: {jb}"
        );
    }

    // ---- Отказы: разные беды — разные ответы --------------------------------

    /// Изменение состояния, не оставившее записи, отклоняется ДО сохранения.
    /// Так ловится самый вероятный будущий дефект: добавили операцию, записать забыли.
    #[tokio::test]
    async fn a_state_change_without_a_journal_entry_is_rejected() {
        let st = build_state();
        let app = router(st.clone());
        // Сначала нормальная операция: журнал заводится и сходится.
        send(
            &app,
            "POST",
            "/pending",
            Some(serde_json::json!({"instrument": 1, "side": "buy", "qty": 1, "price": 10})),
        )
        .await;
        let before = st.broker.lock().await.balance(DEFAULT_USER);

        // А теперь операция, которая меняет счёт в обход журнала — ровно то, что
        // случится, если кто-то добавит путь и не пропишет событие.
        let out: Result<(), ApiErr> = with_account(&st, DEFAULT_USER, Origin::Client, |b| {
            let mut snap = b.snapshot(DEFAULT_USER);
            snap.balance += 777_000;
            b.restore(DEFAULT_USER, snap);
            Ok(())
        })
        .await;
        assert!(out.is_err(), "несписанное изменение обязано быть отклонено");
        assert_eq!(
            st.broker.lock().await.balance(DEFAULT_USER),
            before,
            "и откачено: деньги без проводки не остаются на счёте"
        );
    }

    /// Счёт со слепком, но без журнала, — отдельный диагноз `no_entries`, и он ведёт
    /// в карантин: переиграть такой счёт нечем, а править его деньги вслепую нельзя.
    #[tokio::test]
    async fn an_account_without_a_journal_is_quarantined_as_no_entries() {
        struct SnapshotOnlyStore;
        #[async_trait::async_trait]
        impl persistence::Persistence for SnapshotOnlyStore {
            async fn init(&self) -> Result<(), persistence::StoreError> {
                Ok(())
            }
            async fn load_all(&self) -> Result<persistence::LoadedState, persistence::StoreError> {
                // Инварианты в порядке: расходится именно история, а не состояние.
                let snap = broker::AccountSnapshot {
                    balance: 42_000_000,
                    next_id: 0,
                    ..Default::default()
                };
                Ok(persistence::LoadedState {
                    accounts: vec![(DEFAULT_USER, snap)],
                    journals: vec![],
                })
            }
            async fn save_account(
                &self,
                _u: UserId,
                _s: &broker::AccountSnapshot,
                _e: &[journal::AccountEvent],
                _out: &[persistence::OutboxRecord],
            ) -> Result<(), persistence::StoreError> {
                Ok(())
            }
        }
        let st = build_state_with(Arc::new(SnapshotOnlyStore));
        restore_state(&st)
            .await
            .expect("сервис обязан стартовать: стоп сервиса не применяется нигде");
        assert!(
            st.quarantined.lock().await.contains(&DEFAULT_USER),
            "счёт без истории обязан уйти в карантин"
        );

        let app = router(st);
        let (status, body) = send(&app, "GET", "/account/journal", None).await;
        assert_eq!(status, StatusCode::OK, "читать можно и в карантине");
        assert_eq!(
            body["status"], "no_entries",
            "диагноз обязан называть беду своим именем: {body}"
        );
    }

    /// «Журнал недоступен» — **не** «записей нет». В первом случае мы не знаем истории
    /// и стартовать нельзя; во втором знаем, что её нет, и счёт уходит в карантин, а
    /// сервис работает. Склеить их значило бы работать на выдуманной истории.
    #[tokio::test]
    async fn unreadable_storage_is_not_the_same_as_an_empty_journal() {
        struct UnreadableStore;
        #[async_trait::async_trait]
        impl persistence::Persistence for UnreadableStore {
            async fn init(&self) -> Result<(), persistence::StoreError> {
                Ok(())
            }
            async fn load_all(&self) -> Result<persistence::LoadedState, persistence::StoreError> {
                Err(persistence::StoreError("журнал недоступен".into()))
            }
            async fn save_account(
                &self,
                _u: UserId,
                _s: &broker::AccountSnapshot,
                _e: &[journal::AccountEvent],
                _out: &[persistence::OutboxRecord],
            ) -> Result<(), persistence::StoreError> {
                Ok(())
            }
        }
        let st = build_state_with(Arc::new(UnreadableStore));
        let out = restore_state(&st).await;
        assert!(out.is_err(), "на недоступном журнале сервис не стартует");
        assert!(
            st.quarantined.lock().await.is_empty(),
            "карантин тут ни при чём: мы вообще ничего не прочитали"
        );
    }

    /// Испорченная цепочка в хранилище ловится при подъёме: счёт в карантин, сервис
    /// работает, состояние НЕ чинится — слепок остаётся таким, каким его подняли.
    #[tokio::test]
    async fn a_tampered_journal_quarantines_the_account_and_is_not_repaired() {
        let store = Arc::new(persistence::MemoryStore::new());
        {
            let st = build_state_with(store.clone());
            let app = router(st.clone());
            send(
                &app,
                "POST",
                "/pending",
                Some(serde_json::json!({"instrument": 1, "side": "buy", "qty": 5, "price": 100})),
            )
            .await;
        }
        // Правим ленту так, как её правил бы человек с доступом к базе: меняем
        // содержимое записи, оставив отпечаток прежним.
        struct Tampered(Arc<persistence::MemoryStore>);
        #[async_trait::async_trait]
        impl persistence::Persistence for Tampered {
            async fn init(&self) -> Result<(), persistence::StoreError> {
                Ok(())
            }
            async fn load_all(&self) -> Result<persistence::LoadedState, persistence::StoreError> {
                let mut loaded = self.0.load_all().await?;
                for (_, lane) in loaded.journals.iter_mut() {
                    let v = &lane[0];
                    lane[0] = journal::AccountEvent::from_storage(
                        v.seq(),
                        v.at_ms(),
                        v.origin().clone(),
                        journal::EventKind::AccountOpened {
                            balance: 99_000_000,
                        },
                        v.prev(),
                        v.hash(),
                    );
                }
                Ok(loaded)
            }
            async fn save_account(
                &self,
                u: UserId,
                s: &broker::AccountSnapshot,
                e: &[journal::AccountEvent],
                out: &[persistence::OutboxRecord],
            ) -> Result<(), persistence::StoreError> {
                self.0.save_account(u, s, e, out).await
            }
        }
        let st2 = build_state_with(Arc::new(Tampered(store.clone())));
        restore_state(&st2).await.expect("сервис обязан стартовать");
        assert!(
            st2.quarantined.lock().await.contains(&DEFAULT_USER),
            "подделанная лента — карантин"
        );

        let app2 = router(st2.clone());
        let (status, _) = send(
            &app2,
            "POST",
            "/pending",
            Some(serde_json::json!({"instrument": 1, "side": "buy", "qty": 1, "price": 1})),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::LOCKED,
            "мутации по счёту с испорченной историей не проходят"
        );
        let snap = st2.broker.lock().await.snapshot(DEFAULT_USER);
        assert_eq!(
            snap.pendings.len(),
            1,
            "состояние не чинится и не затирается: улика остаётся"
        );
    }

    /// Отказ хранилища не оставляет записи в журнале: иначе в памяти была бы история,
    /// которой нет в базе, и после рестарта она исчезла бы молча.
    #[tokio::test]
    async fn a_failed_write_leaves_no_entry_in_the_journal() {
        struct FailingStore;
        #[async_trait::async_trait]
        impl persistence::Persistence for FailingStore {
            async fn init(&self) -> Result<(), persistence::StoreError> {
                Ok(())
            }
            async fn load_all(&self) -> Result<persistence::LoadedState, persistence::StoreError> {
                Ok(persistence::LoadedState::default())
            }
            async fn save_account(
                &self,
                _u: UserId,
                _s: &broker::AccountSnapshot,
                _e: &[journal::AccountEvent],
                _out: &[persistence::OutboxRecord],
            ) -> Result<(), persistence::StoreError> {
                Err(persistence::StoreError("диск горит".into()))
            }
        }
        let st = build_state_with(Arc::new(FailingStore));
        let app = router(st.clone());
        let (status, _) = send(
            &app,
            "POST",
            "/pending",
            Some(serde_json::json!({"instrument": 1, "side": "buy", "qty": 1, "price": 10})),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            events_of(&st).await.is_empty(),
            "незаписанного в хранилище в журнале быть не должно"
        );
    }
}

/// Сделка из терминала доезжает до исходящей очереди (TRM-40).
///
/// Проверка идёт **через роутер и через монитор**, а не по `bus::envelopes_for`
/// напрямую. Сборка конверта проверена отдельно и в вакууме; здесь проверяется
/// другое — что барьер стоит на своём месте, на обоих путях мутации счёта. Путь,
/// собирающий событие только у ручных операций, молчит ровно про сработавший стоп,
/// а спорят с клиентом как раз о нём (урок BUG-013, он же TRM-13).
#[cfg(test)]
mod bus_wiring_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const TENANT: &str = "apex-ru";
    const SUB: &str = "sub-777";

    /// Хранилище, у которого счёт привязан к сайту.
    ///
    /// `MemoryStore` личностей не знает и честно отвечает `None` — а без адресата
    /// события не собираются вовсе. Поэтому для проверки публикации нужна привязка,
    /// и она здесь настоящая, а не «всегда да»: чужой счёт остаётся без личности,
    /// и это проверяется отдельным случаем.
    struct Tenanted {
        inner: persistence::MemoryStore,
        linked: UserId,
    }

    #[async_trait::async_trait]
    impl Persistence for Tenanted {
        async fn init(&self) -> Result<(), persistence::StoreError> {
            self.inner.init().await
        }
        async fn load_all(&self) -> Result<persistence::LoadedState, persistence::StoreError> {
            self.inner.load_all().await
        }
        async fn save_account(
            &self,
            u: UserId,
            s: &broker::AccountSnapshot,
            e: &[journal::AccountEvent],
            o: &[persistence::OutboxRecord],
        ) -> Result<(), persistence::StoreError> {
            self.inner.save_account(u, s, e, o).await
        }
        async fn claim_outbox(
            &self,
            limit: i64,
            lease_ms: i64,
        ) -> Result<Vec<persistence::OutboxClaim>, persistence::StoreError> {
            self.inner.claim_outbox(limit, lease_ms).await
        }
        async fn mark_published(&self, id: i64) -> Result<(), persistence::StoreError> {
            self.inner.mark_published(id).await
        }
        async fn outbox_backlog(&self) -> Result<i64, persistence::StoreError> {
            self.inner.outbox_backlog().await
        }
        async fn identity_of(
            &self,
            u: UserId,
        ) -> Result<Option<(String, String)>, persistence::StoreError> {
            Ok((u == self.linked).then(|| (TENANT.to_string(), SUB.to_string())))
        }
    }

    fn tenanted(linked: UserId) -> Arc<Tenanted> {
        Arc::new(Tenanted {
            inner: persistence::MemoryStore::new(),
            linked,
        })
    }

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
    ) -> StatusCode {
        let b = Request::builder().method(method).uri(uri);
        let req = match body {
            Some(v) => b
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&v).unwrap()))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let _ = resp.into_body().collect().await.unwrap();
        status
    }

    async fn with_price(st: &AppState, instrument: u32, px: i64) {
        st.tickers.lock().await.insert(
            instrument,
            feed::Ticker {
                last: px,
                change: 0.0,
                high: px,
                bid: px,
                ask: px,
            },
        );
    }

    /// Все конверты, лежащие в очереди, разобранные обратно в JSON.
    async fn queued(st: &AppState) -> Vec<serde_json::Value> {
        st.store
            .claim_outbox(100, 0)
            .await
            .expect("очередь обязана читаться")
            .iter()
            .map(|c| serde_json::from_str(&c.payload).expect("конверт — валидный JSON"))
            .collect()
    }

    fn of_kind<'a>(
        all: &'a [serde_json::Value],
        name: &str,
    ) -> Vec<&'a serde_json::Value> {
        all.iter().filter(|e| e["event"] == name).collect()
    }

    /// Сработавший стоп уезжает на шину — с ценой исполнения, уровнем и разрывом.
    #[tokio::test]
    async fn a_stop_loss_reaches_the_outgoing_queue() {
        let st = build_state_with(tenanted(DEFAULT_USER));
        restore_state(&st).await.unwrap();
        with_price(&st, 1, 5_000_000).await; // BTCUSD, pd = 2 → 50 000.00
        let app = router(st.clone());

        let body = serde_json::json!({
            "instrument": 1, "side": "buy", "qty": 100_000, "sl": 4_900_000
        });
        assert_eq!(send(&app, "POST", "/deals", Some(body)).await, StatusCode::OK);

        // Рынок перепрыгнул стоп: исполнение по 48 000.00, уровень был 49 000.00.
        with_price(&st, 1, 4_800_000).await;
        monitor_tick(&st).await;

        let all = queued(&st).await;
        let trades = of_kind(&all, bus::EV_TRADE_EXECUTED);
        assert_eq!(trades.len(), 1, "сделка обязана уйти наружу: {all:?}");
        let d = &trades[0]["data"];
        assert_eq!(d["tenant"], TENANT);
        assert_eq!(d["userId"], SUB);
        assert_eq!(d["symbol"], "BTCUSD");
        assert_eq!(d["price"], serde_json::json!(48_000.0), "цена ИСПОЛНЕНИЯ");
        assert_eq!(
            d["level"],
            serde_json::json!(49_000.0),
            "и поставленный уровень — без него проскальзывание не посчитать"
        );
        assert_eq!(d["acrossGap"], serde_json::json!(false));
        assert_eq!(trades[0]["source"], bus::SOURCE);
        assert_eq!(trades[0]["version"], bus::EVENT_VERSION);

        // Остаток изменился — об этом отдельное событие, и в нём новый остаток.
        let balances = of_kind(&all, bus::EV_BALANCE_CHANGED);
        assert_eq!(balances.len(), 1, "изменение остатка обязано уехать");
        assert_eq!(balances[0]["data"]["reason"], "trade");
    }

    /// Заведение счёта ручным открытием — тоже событие, и оно первое.
    #[tokio::test]
    async fn opening_an_account_reaches_the_queue_before_the_trade() {
        let st = build_state_with(tenanted(DEFAULT_USER));
        restore_state(&st).await.unwrap();
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());
        assert_eq!(
            send(
                &app,
                "POST",
                "/deals",
                Some(serde_json::json!({"instrument": 1, "side": "buy", "qty": 100_000}))
            )
            .await,
            StatusCode::OK
        );

        let all = queued(&st).await;
        assert_eq!(
            all.first().map(|e| e["event"].clone()),
            Some(serde_json::json!(bus::EV_ACCOUNT_OPENED)),
            "счёт заводится раньше, чем по нему торгуют: {all:?}"
        );
    }

    /// **Отказ записи — нет события.** 503 клиенту и пустая очередь: наружу не
    /// должна уехать сделка, которой нет в хранилище.
    #[tokio::test]
    async fn a_failed_write_publishes_nothing() {
        struct Burning(persistence::MemoryStore);
        #[async_trait::async_trait]
        impl Persistence for Burning {
            async fn init(&self) -> Result<(), persistence::StoreError> {
                Ok(())
            }
            async fn load_all(&self) -> Result<persistence::LoadedState, persistence::StoreError> {
                Ok(Default::default())
            }
            async fn save_account(
                &self,
                _u: UserId,
                _s: &broker::AccountSnapshot,
                _e: &[journal::AccountEvent],
                _o: &[persistence::OutboxRecord],
            ) -> Result<(), persistence::StoreError> {
                Err(persistence::StoreError("диск горит".into()))
            }
            async fn claim_outbox(
                &self,
                limit: i64,
                lease_ms: i64,
            ) -> Result<Vec<persistence::OutboxClaim>, persistence::StoreError> {
                self.0.claim_outbox(limit, lease_ms).await
            }
            async fn outbox_backlog(&self) -> Result<i64, persistence::StoreError> {
                self.0.outbox_backlog().await
            }
            async fn identity_of(
                &self,
                _u: UserId,
            ) -> Result<Option<(String, String)>, persistence::StoreError> {
                Ok(Some((TENANT.to_string(), SUB.to_string())))
            }
        }

        let st = build_state_with(Arc::new(Burning(persistence::MemoryStore::new())));
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());
        assert_eq!(
            send(
                &app,
                "POST",
                "/deals",
                Some(serde_json::json!({"instrument": 1, "side": "buy", "qty": 100_000}))
            )
            .await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            st.store.outbox_backlog().await.unwrap(),
            0,
            "события не существует отдельно от денег"
        );
    }

    /// Счёт без привязки к сайту (standalone) — адресата нет, и событий нет.
    /// Не ошибка: адресовать их некому, а «кому-нибудь» мы не шлём.
    #[tokio::test]
    async fn an_unlinked_account_publishes_nothing() {
        let st = build_state_with(tenanted(UserId(999))); // привязан ДРУГОЙ счёт
        restore_state(&st).await.unwrap();
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());
        assert_eq!(
            send(
                &app,
                "POST",
                "/deals",
                Some(serde_json::json!({"instrument": 1, "side": "buy", "qty": 100_000}))
            )
            .await,
            StatusCode::OK,
            "торговля не зависит от того, доедет ли событие"
        );
        assert_eq!(st.store.outbox_backlog().await.unwrap(), 0);
    }

    /// `/health` называет и состояние шины, и длину очереди: «копится» и
    /// «копится, потому что слать некуда» — разные беды и разные люди.
    #[tokio::test]
    async fn health_tells_whether_the_bus_is_configured_and_how_much_waits() {
        let st = build_state_with(tenanted(DEFAULT_USER));
        restore_state(&st).await.unwrap();
        with_price(&st, 1, 5_000_000).await;
        let app = router(st.clone());
        let _ = send(
            &app,
            "POST",
            "/deals",
            Some(serde_json::json!({"instrument": 1, "side": "buy", "qty": 100_000})),
        )
        .await;

        let resp = app
            .clone()
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["bus"], false, "в тестах шина не настроена, и это видно");
        assert_eq!(
            v["outbox_pending"], 1,
            "и видно, сколько ждёт отправки: {v}"
        );
    }
}
