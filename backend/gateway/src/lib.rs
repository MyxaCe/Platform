//! # gateway
//!
//! Сетевой шлюз биржи: REST + WS. Рыночные данные — **реальные с Binance** (ADR-013): `/instruments`
//! и `/candles` берут живые котировки/свечи из [`feed`]; лента и live-цена идут через WS `/stream`.
//! Matching-ядро/ledger/оркестратор сохранены (эндпоинты `/orders`,`/book`,`/balance`) как фундамент
//! для будущего broker-исполнения. Доменные типы наружу не протекают — на границе локальные DTO.

pub mod feed;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
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
use domain::instrument::{AssetId, Instrument, InstrumentId};
use domain::money::{Amount, Price, Qty};
use domain::order::{OrderId, Side, TimeInForce};

use broker::{Broker, PosSide};
use persistence::{NoopStore, Persistence};

pub mod mailer;
pub mod passkey;

/// Реэкспорт: тестам и бинарю нужен трейт хранилища, чтобы подставить свою реализацию.
pub use persistence;
use exchange_core::Event;
use orchestrator::{OrderReject, Orchestrator};

// ============================ Состояние ====================================

/// Реестр пользователей и сессий.
///
/// `by_token` — прежние dev-токены (alice-token/bob-token), живут до переезда терминала
/// на сессии (ADR-018). `by_email` — настоящие учётные записи: почта → (id, PHC-хэш).
/// `sessions` — хэш токена сессии → (чей, до какого времени в unix-секундах).
/// Память — рабочее состояние, БД — durable-проекция, как и у брокера (ADR-016).
#[derive(Debug, Default)]
pub struct UserRegistry {
    by_token: HashMap<String, UserId>,
    by_email: HashMap<String, (UserId, String)>,
    sessions: HashMap<String, (UserId, i64)>,
    /// Кто подтвердил почту кодом. Пока в памяти: до реальных денег переедет в БД
    /// вместе с остальным профилем.
    verified: std::collections::HashSet<UserId>,
    /// Публичные ключи Passkey по пользователям (ADR-020). Приватные ключи — только
    /// на устройстве клиента; у нас лежит то, чем проверяется подпись.
    passkeys: HashMap<UserId, Vec<webauthn_rs::prelude::Passkey>>,
    next: u64,
}
impl UserRegistry {
    pub fn create(&mut self, token: String) -> UserId {
        self.next += 1;
        let id = UserId(self.next);
        self.by_token.insert(token, id);
        id
    }
    pub fn resolve(&self, token: &str) -> Option<UserId> {
        self.by_token.get(token).copied()
    }
    /// Восстановить пользователя из хранилища на старте (ADR-016). `next` держим выше
    /// максимального загруженного id, иначе новый пользователь получил бы занятый id.
    pub fn restore(&mut self, token: String, id: UserId) {
        self.next = self.next.max(id.0);
        self.by_token.insert(token, id);
    }
    // ---- Учётные записи и сессии (ADR-018) --------------------------------

    /// Завести учётную запись. Почта уже нормализована вызывающим.
    pub fn create_with_password(&mut self, email: String, password_hash: String) -> UserId {
        self.next += 1;
        let id = UserId(self.next);
        self.by_email.insert(email, (id, password_hash));
        id
    }
    /// Поднять учётные данные из хранилища на старте.
    pub fn restore_credentials(&mut self, id: UserId, email: String, password_hash: String) {
        self.next = self.next.max(id.0);
        self.by_email.insert(email, (id, password_hash));
    }
    pub fn find_by_email(&self, email: &str) -> Option<(UserId, String)> {
        self.by_email.get(email).map(|(id, h)| (*id, h.clone()))
    }
    pub fn email_of(&self, user: UserId) -> Option<String> {
        self.by_email.iter().find(|(_, (id, _))| *id == user).map(|(e, _)| e.clone())
    }
    pub fn put_session(&mut self, token_hash: String, user: UserId, expires: i64) {
        self.sessions.insert(token_hash, (user, expires));
    }
    /// Сессия по хэшу токена; просроченная не считается действующей.
    pub fn session(&self, token_hash: &str, now: i64) -> Option<UserId> {
        self.sessions.get(token_hash).filter(|(_, exp)| *exp > now).map(|(id, _)| *id)
    }
    pub fn drop_session(&mut self, token_hash: &str) {
        self.sessions.remove(token_hash);
    }
    pub fn mark_verified(&mut self, user: UserId) {
        self.verified.insert(user);
    }
    pub fn is_verified(&self, user: UserId) -> bool {
        self.verified.contains(&user)
    }
    pub fn add_passkey(&mut self, user: UserId, pk: webauthn_rs::prelude::Passkey) {
        self.passkeys.entry(user).or_default().push(pk);
    }
    pub fn passkeys_of(&self, user: UserId) -> Vec<webauthn_rs::prelude::Passkey> {
        self.passkeys.get(&user).cloned().unwrap_or_default()
    }
    pub fn set_passkeys(&mut self, user: UserId, keys: Vec<webauthn_rs::prelude::Passkey>) {
        self.passkeys.insert(user, keys);
    }

    /// Идемпотентно: известный токен возвращает свой id. Флаг — создан ли новый.
    ///
    /// Нужно для демо-сида: он выполняется на каждом старте, и безусловный `create`
    /// после восстановления из БД выдавал токену НОВЫЙ id — счёт со сделками
    /// оставался в базе, но становился недоступен (поймано на рестарте).
    pub fn get_or_create(&mut self, token: String) -> (UserId, bool) {
        match self.by_token.get(&token) {
            Some(id) => (*id, false),
            None => (self.create(token), true),
        }
    }
}

/// Кэш свечей: (инструмент, таймфрейм) → (время загрузки, свечи).
type KlineCache = HashMap<(u32, u32), (Instant, Vec<feed::Kline>)>;
/// Кэш стакана: (инструмент, запрошенная глубина) → (время, стакан).
type DepthCache = HashMap<(u32, usize), (Instant, feed::Depth)>;
/// Кэш статистики: инструмент → (время, изменения по таймфреймам).
type StatsCache = HashMap<u32, (Instant, Vec<StatDto>)>;

/// Таймфреймы для статистики актива.
const STAT_TFS: [u32; 8] = [60, 300, 900, 1800, 3600, 14400, 86400, 604800];

#[derive(Clone)]
pub struct AppState {
    orch: Arc<Mutex<Orchestrator>>,
    users: Arc<Mutex<UserRegistry>>,
    order_seq: Arc<AtomicU64>,
    events_tx: broadcast::Sender<String>,
    // Реальные данные (Binance):
    feed_instruments: Arc<Vec<feed::FeedInstrument>>,
    tickers: Arc<Mutex<HashMap<u32, feed::Ticker>>>,
    klines_cache: Arc<Mutex<KlineCache>>,
    depth_cache: Arc<Mutex<DepthCache>>,
    stats_cache: Arc<Mutex<StatsCache>>,
    // Бумажный брокер (ADR-014):
    broker: Arc<Mutex<Broker>>,
    // Durable-хранилище (ADR-016). Память — рабочее состояние, хранилище — источник
    // восстановления; сам брокер о БД не знает.
    store: Arc<dyn Persistence>,
    /// Лок на счёт: сериализует «мутация → слепок → запись» по одному счёту.
    /// Разные счета не мешают друг другу.
    acct_locks: Arc<Mutex<HashMap<UserId, Arc<Mutex<()>>>>>,
    /// Коды подтверждения почты: пользователь → (код, до какого времени). В памяти:
    /// живут минуты и переживать перезапуск не обязаны.
    codes: Arc<Mutex<HashMap<UserId, (String, i64)>>>,
    /// Попытки входа и регистрации для защиты от перебора (ADR-018): ключ → отметки времени.
    /// Живёт в памяти процесса; при распиле на сервисы переедет в общее хранилище.
    attempts: Arc<Mutex<HashMap<String, Vec<i64>>>>,
    /// WebAuthn-движок (ADR-020) и незавершённые церемонии passkey (flow-id → состояние).
    webauthn: Arc<webauthn_rs::Webauthn>,
    pk_flows: Arc<Mutex<HashMap<String, (passkey::PkFlow, i64)>>>,
}

pub fn build_state() -> AppState {
    build_state_with(Arc::new(NoopStore))
}

/// Собрать состояние с конкретным хранилищем. `NoopStore` = поведение до ADR-016
/// (всё в памяти) — на нём работают существующие тесты и запуск без БД.
pub fn build_state_with(store: Arc<dyn Persistence>) -> AppState {
    let (events_tx, _rx) = broadcast::channel(4096);
    AppState {
        orch: Arc::new(Mutex::new(Orchestrator::new())),
        users: Arc::new(Mutex::new(UserRegistry::default())),
        order_seq: Arc::new(AtomicU64::new(1)),
        events_tx,
        feed_instruments: Arc::new(feed::instruments()),
        tickers: Arc::new(Mutex::new(HashMap::new())),
        klines_cache: Arc::new(Mutex::new(HashMap::new())),
        depth_cache: Arc::new(Mutex::new(HashMap::new())),
        stats_cache: Arc::new(Mutex::new(HashMap::new())),
        broker: Arc::new(Mutex::new(Broker::new(10_000_000, 1))), // старт $100k, leverage 1
        store,
        acct_locks: Arc::new(Mutex::new(HashMap::new())),
        codes: Arc::new(Mutex::new(HashMap::new())),
        attempts: Arc::new(Mutex::new(HashMap::new())),
        webauthn: passkey::build_webauthn(),
        pk_flows: Arc::new(Mutex::new(HashMap::new())),
    }
}

/// Поднять состояние из хранилища. Вызывается один раз на старте, до приёма запросов.
pub async fn restore_state(st: &AppState) -> Result<(), persistence::StoreError> {
    st.store.init().await?;
    let loaded = st.store.load_all().await?;
    let (users_n, accounts_n) = (loaded.users.len(), loaded.accounts.len());
    let (creds_n, sess_n, pk_n) = (loaded.credentials.len(), loaded.sessions.len(), loaded.passkeys.len());
    {
        let mut reg = st.users.lock().await;
        for (id, token) in loaded.users {
            reg.restore(token, id);
        }
        for (id, email, hash) in loaded.credentials {
            reg.restore_credentials(id, email, hash);
        }
        for (token_hash, id, expires) in loaded.sessions {
            reg.put_session(token_hash, id, expires);
        }
        for (id, data) in loaded.passkeys {
            match serde_json::from_str(&data) {
                Ok(pk) => reg.add_passkey(id, pk),
                Err(e) => eprintln!("[passkey] не разобрал сохранённый ключ пользователя {}: {e}", id.0),
            }
        }
    }
    {
        let mut b = st.broker.lock().await;
        for (id, snap) in loaded.accounts {
            b.restore(id, snap);
        }
    }
    // Красная линия №5: инварианты денег проверяются в рантайме, нарушение = стоп.
    let broken = st.store.check_invariants().await?;
    if !broken.is_empty() {
        return Err(persistence::StoreError(format!("нарушены инварианты: {}", broken.join("; "))));
    }
    if users_n + accounts_n + creds_n + sess_n + pk_n > 0 {
        println!("[gateway] восстановлено из хранилища: пользователей {users_n}, счетов {accounts_n}, учётных записей {creds_n}, сессий {sess_n}, passkey {pk_n}");
    }
    Ok(())
}

/// Лок конкретного счёта (создаётся лениво).
async fn account_lock(st: &AppState, user: UserId) -> Arc<Mutex<()>> {
    st.acct_locks.lock().await.entry(user).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
}

/// Мутация счёта с сохранением (ADR-016).
///
/// Дисциплина:
/// 1. взять **лок счёта** — он держится на всё «мутация → слепок → запись». Без него
///    две конкурентные операции по одному счёту могли записаться в обратном порядке
///    (монитор дёрнул SL, пока пользователь закрывал позицию), и в БД осталось бы
///    устаревшее состояние. Разные счета друг друга не ждут;
/// 2. под локом брокера мутировать и снять слепки «до» и «после»;
/// 3. **отпустить лок брокера** и только тогда писать: `fsync`/сеть под ним
///    заблокировали бы торговлю всех пользователей;
/// 4. не записалось — вернуть счёт к слепку «до» и отдать `503`: клиент не увидит
///    `200` по операции, которой нет на диске.
async fn with_account<T>(st: &AppState, user: UserId, f: impl FnOnce(&mut Broker) -> Result<T, ApiErr>) -> Result<T, ApiErr> {
    let lock = account_lock(st, user).await;
    let _guard = lock.lock().await;

    let (out, before, snap) = {
        let mut b = st.broker.lock().await;
        let before = b.snapshot(user);
        let out = f(&mut b)?; // операция не прошла — ничего не мутировали, писать нечего
        let snap = b.snapshot(user);
        (out, before, snap)
    };

    if let Err(e) = st.store.save_account(user, &snap).await {
        st.broker.lock().await.restore(user, before);
        eprintln!("[gateway] запись состояния не удалась, счёт откачен: {e}");
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable"));
    }
    Ok(out)
}

/// Текущие марк-цены (инструмент → last) из фида — для P&L/equity брокера.
async fn marks(st: &AppState) -> HashMap<u32, i64> {
    st.tickers.lock().await.iter().map(|(id, t)| (*id, t.last)).collect()
}

/// Разослать события в живую ленту.
async fn publish(st: &AppState, events: &[Event]) {
    let dtos: Vec<EventDto> = events.iter().map(EventDto::from).collect();
    if let Ok(json) = serde_json::to_string(&dtos) {
        let _ = st.events_tx.send(json);
    }
}

// ============================ Роутер =======================================

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/auth/check-email", post(check_email))
        .route("/auth/send-code", post(send_code))
        .route("/auth/verify-code", post(verify_code))
        .route("/auth/passkey/register/start", post(passkey::register_start))
        .route("/auth/passkey/register/finish", post(passkey::register_finish))
        .route("/auth/passkey/login/start", post(passkey::login_start))
        .route("/auth/passkey/login/finish", post(passkey::login_finish))
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .route("/admin/users", post(create_user))
        .route("/admin/deposit", post(deposit))
        .route("/instruments", get(list_instruments))
        .route("/candles/:instrument", get(get_candles))
        .route("/depth/:instrument", get(get_depth))
        .route("/stats/:instrument", get(get_stats))
        .route("/orders", post(place_order))
        .route("/orders/:id", delete(cancel_order))
        .route("/book/:instrument", get(get_book))
        .route("/balance/:asset", get(get_balance))
        .route("/deals", post(open_deal).get(list_deals))
        .route("/deals/closed", get(list_closed))
        .route("/deals/:id/close", post(close_deal))
        .route("/pending", post(place_pending).get(list_pending))
        .route("/pending/:id", delete(cancel_pending))
        .route("/account", get(account))
        .route("/stream", get(ws_stream))
        .with_state(state)
}

// ============================ DTO ==========================================

#[derive(Serialize)]
struct ErrResp {
    error: String,
}
type ApiErr = (StatusCode, Json<ErrResp>);
fn err(code: StatusCode, msg: &str) -> ApiErr {
    (code, Json(ErrResp { error: msg.to_string() }))
}

#[derive(Deserialize)]
struct CreateUserReq {
    token: String,
}
#[derive(Serialize)]
struct CreateUserResp {
    user_id: u64,
}
#[derive(Deserialize)]
struct DepositReq {
    user_id: u64,
    asset: u32,
    amount: i128,
}
#[derive(Serialize)]
struct BalanceResp {
    asset: u32,
    available: i128,
    held: i128,
}
#[derive(Deserialize)]
struct PlaceOrderReq {
    instrument: u32,
    side: String,
    #[serde(rename = "type")]
    order_type: String,
    price: Option<i64>,
    qty: i64,
    tif: Option<String>,
}
#[derive(Serialize)]
struct PlaceResp {
    order_id: u64,
    events: Vec<EventDto>,
}
#[derive(Deserialize)]
struct CancelQuery {
    instrument: u32,
}
#[derive(Deserialize)]
struct BookQuery {
    depth: Option<usize>,
}
#[derive(Serialize)]
struct LevelDto {
    price: i64,
    qty: i64,
    orders: u32,
}
#[derive(Serialize)]
struct BookResp {
    bids: Vec<LevelDto>,
    asks: Vec<LevelDto>,
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
    exit: i64,
    pnl: i128,
    price_decimals: u8,
    qty_decimals: u8,
}
#[derive(Serialize)]
struct AccountDto {
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
    change: f64,
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

#[derive(Serialize, Clone)]
struct StatDto {
    tf: u32,
    change: f64,
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
enum EventDto {
    OrderAccepted { instrument: u32, id: u64 },
    Trade { instrument: u32, trade_id: u64, price: i64, qty: i64, taker: u64, maker: u64, taker_side: String },
    OrderResting { instrument: u32, id: u64, price: i64, qty: i64 },
    OrderFilled { instrument: u32, id: u64 },
    OrderCanceledRemainder { instrument: u32, id: u64, qty: i64 },
    OrderCanceled { instrument: u32, id: u64 },
    OrderRejected { instrument: u32, id: u64, reason: String },
}
impl From<&Event> for EventDto {
    fn from(e: &Event) -> Self {
        match e {
            Event::OrderAccepted { instrument, id } => EventDto::OrderAccepted { instrument: instrument.0, id: id.0 },
            Event::Trade { instrument, id, price, qty, taker, maker, taker_side } => EventDto::Trade {
                instrument: instrument.0,
                trade_id: id.0,
                price: price.0,
                qty: qty.0,
                taker: taker.0,
                maker: maker.0,
                taker_side: side_str(*taker_side).to_string(),
            },
            Event::OrderResting { instrument, id, price, qty } => EventDto::OrderResting { instrument: instrument.0, id: id.0, price: price.0, qty: qty.0 },
            Event::OrderFilled { instrument, id } => EventDto::OrderFilled { instrument: instrument.0, id: id.0 },
            Event::OrderCanceledRemainder { instrument, id, qty } => EventDto::OrderCanceledRemainder { instrument: instrument.0, id: id.0, qty: qty.0 },
            Event::OrderCanceled { instrument, id } => EventDto::OrderCanceled { instrument: instrument.0, id: id.0 },
            Event::OrderRejected { instrument, id, reason } => EventDto::OrderRejected { instrument: instrument.0, id: id.0, reason: format!("{reason:?}") },
        }
    }
}
fn side_str(s: Side) -> &'static str {
    match s {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}
fn reject_to_http(r: OrderReject) -> ApiErr {
    match r {
        OrderReject::InsufficientFunds => err(StatusCode::PAYMENT_REQUIRED, "insufficient_funds"),
        OrderReject::NotOwner => err(StatusCode::FORBIDDEN, "not_owner"),
        OrderReject::SelfTrade => err(StatusCode::CONFLICT, "self_trade"),
        OrderReject::Engine(reason) => err(StatusCode::BAD_REQUEST, &format!("engine:{reason:?}")),
    }
}

// ============================ Хендлеры =====================================

async fn health() -> &'static str {
    "ok"
}


// ============================ Аутентификация (ADR-018) =====================

const SESSION_COOKIE: &str = "session";
const SESSION_TTL_SECS: i64 = 30 * 24 * 3600; // 30 дней

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Deserialize)]
struct CredentialsReq {
    email: String,
    password: String,
}

#[derive(Serialize)]
struct MeResp {
    user_id: u64,
    email: String,
    /// Подтверждена ли почта кодом. Пока нет — кабинет ограничивает операции.
    verified: bool,
}

/// Достать значение cookie из заголовка.
fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(axum::http::header::COOKIE)?.to_str().ok()?.split(';').find_map(|p| {
        let (k, v) = p.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

/// Заголовок с сессионной cookie. `HttpOnly` — чтобы её не достал JavaScript при XSS;
/// `Secure` включается переменной COOKIE_SECURE, когда сервис работает за HTTPS.
fn session_cookie(token: &str, max_age: i64) -> String {
    let secure = if std::env::var("COOKIE_SECURE").is_ok() { "; Secure" } else { "" };
    // Домен нужен, чтобы сессия действовала на всех поддоменах продукта
    // (accounts/trade/my), а не только там, где выполнен вход (ADR-019).
    let domain = match std::env::var("COOKIE_DOMAIN") {
        Ok(d) if !d.is_empty() => format!("; Domain={d}"),
        _ => String::new(),
    };
    format!("{SESSION_COOKIE}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}{secure}{domain}")
}

/// Скользящее окно попыток. `true` — лимит исчерпан.
async fn too_many(st: &AppState, key: &str, limit: usize, window: i64) -> bool {
    let now = now_unix();
    let mut map = st.attempts.lock().await;
    let hits = map.entry(key.to_string()).or_default();
    hits.retain(|t| now - *t < window);
    if hits.len() >= limit {
        return true;
    }
    hits.push(now);
    false
}

/// Адрес клиента для ограничителя. За прокси берём X-Forwarded-For.
fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .unwrap_or("unknown")
        .trim()
        .to_string()
}

/// Открыть сессию: положить в память, записать в хранилище, вернуть заголовок cookie.
async fn open_session(st: &AppState, user: UserId) -> Result<String, ApiErr> {
    let (token, token_hash) = auth::new_session_token();
    let now = now_unix();
    let expires = now + SESSION_TTL_SECS;
    st.users.lock().await.put_session(token_hash.clone(), user, expires);
    if let Err(e) = st.store.save_session(&token_hash, user, now, expires).await {
        eprintln!("[auth] сессия не сохранена: {e}");
    }
    Ok(session_cookie(&token, SESSION_TTL_SECS))
}

#[derive(Deserialize)]
struct EmailReq {
    email: String,
}

#[derive(Serialize)]
struct EmailCheckResp {
    /// Зарегистрирована ли почта: сайт по этому ответу решает, спрашивать пароль
    /// (вход) или заводить учётку (регистрация).
    registered: bool,
    /// Есть ли у пользователя привязанный Passkey — тогда предлагаем вход по ключу.
    has_passkey: bool,
}

/// Шаг 1 пошагового входа: существует ли учётка с такой почтой.
///
/// Осознанный компромисс: ответ раскрывает, зарегистрирован ли адрес. Так устроен
/// вход у большинства крупных сервисов — иначе пошаговую форму не сделать. Риск
/// (перебор адресов) гасится ограничителем: 20 проверок за 5 минут с одного IP.
async fn check_email(State(st): State<AppState>, headers: HeaderMap, Json(req): Json<EmailReq>) -> Result<Json<EmailCheckResp>, ApiErr> {
    if too_many(&st, &format!("check:{}", client_ip(&headers)), 20, 300).await {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too many attempts, try later"));
    }
    let email = auth::normalize_email(&req.email);
    if !auth::is_email_like(&email) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid email"));
    }
    let (registered, has_passkey) = {
        let reg = st.users.lock().await;
        match reg.find_by_email(&email) {
            Some((id, _)) => (true, !reg.passkeys_of(id).is_empty()),
            None => (false, false),
        }
    };
    Ok(Json(EmailCheckResp { registered, has_passkey }))
}

#[derive(Deserialize)]
struct VerifyReq {
    code: String,
}

/// Выслать код подтверждения на почту текущего пользователя.
async fn send_code(State(st): State<AppState>, headers: HeaderMap) -> Result<Json<serde_json::Value>, ApiErr> {
    let user = authed(&st, &headers).await?;
    if too_many(&st, &format!("code:{}", user.0), 5, 600).await {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too many attempts, try later"));
    }
    let email = st.users.lock().await.email_of(user).unwrap_or_default();
    let code = auth::new_verification_code();
    st.codes.lock().await.insert(user, (code.clone(), now_unix() + 900)); // 15 минут
    mailer::send_code(&email, &code).await;
    // Признак `delivered` показывает интерфейсу, ушло ли письмо на самом деле.
    // Без настроенного SMTP код уходит в лог сервера, и врать об этом нельзя.
    Ok(Json(serde_json::json!({ "sent": true, "delivered": mailer::is_configured() })))
}

/// Проверить код подтверждения.
async fn verify_code(State(st): State<AppState>, headers: HeaderMap, Json(req): Json<VerifyReq>) -> Result<Json<serde_json::Value>, ApiErr> {
    let user = authed(&st, &headers).await?;
    if too_many(&st, &format!("verify:{}", user.0), 10, 600).await {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too many attempts, try later"));
    }
    let ok = {
        let codes = st.codes.lock().await;
        codes.get(&user).is_some_and(|(c, exp)| *c == req.code.trim() && *exp > now_unix())
    };
    if !ok {
        return Err(err(StatusCode::BAD_REQUEST, "invalid or expired code"));
    }
    st.codes.lock().await.remove(&user);
    st.users.lock().await.mark_verified(user);
    Ok(Json(serde_json::json!({ "verified": true })))
}

async fn register(State(st): State<AppState>, headers: HeaderMap, Json(req): Json<CredentialsReq>) -> Result<impl IntoResponse, ApiErr> {
    if too_many(&st, &format!("reg:{}", client_ip(&headers)), 5, 3600).await {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too many attempts, try later"));
    }
    let email = auth::normalize_email(&req.email);
    if !auth::is_email_like(&email) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid email"));
    }
    // Требования к паролю проверяются на сервере: проверку в браузере легко обойти.
    auth::check_password_policy(&req.password).map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
    if st.users.lock().await.find_by_email(&email).is_some() {
        return Err(err(StatusCode::CONFLICT, "email already registered"));
    }
    let hash = auth::hash_password(&req.password).map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "hash failed"))?;
    let user = { st.users.lock().await.create_with_password(email.clone(), hash.clone()) };
    if let Err(e) = st.store.save_credentials(user, &email, &hash).await {
        eprintln!("[auth] учётные данные не сохранены: {e}");
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable"));
    }
    let ck = open_session(&st, user).await?;
    Ok(([(axum::http::header::SET_COOKIE, ck)], Json(MeResp { user_id: user.0, email, verified: false })))
}

async fn login(State(st): State<AppState>, headers: HeaderMap, Json(req): Json<CredentialsReq>) -> Result<impl IntoResponse, ApiErr> {
    let email = auth::normalize_email(&req.email);
    if too_many(&st, &format!("login:{}:{}", client_ip(&headers), email), 10, 300).await {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too many attempts, try later"));
    }
    let found = st.users.lock().await.find_by_email(&email);
    // Хэш проверяется всегда, даже если почты нет: иначе по времени ответа видно,
    // какие адреса зарегистрированы. Сообщение об ошибке тоже одно на оба случая.
    let (user, hash) = match found {
        Some(v) => v,
        None => (UserId(0), auth::dummy_hash().to_string()),
    };
    if !auth::verify_password(&req.password, &hash) || user.0 == 0 {
        return Err(err(StatusCode::UNAUTHORIZED, "invalid email or password"));
    }
    let ck = open_session(&st, user).await?;
    let verified = st.users.lock().await.is_verified(user);
    Ok(([(axum::http::header::SET_COOKIE, ck)], Json(MeResp { user_id: user.0, email, verified })))
}

async fn logout(State(st): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if let Some(token) = cookie(&headers, SESSION_COOKIE) {
        let h = auth::hash_session_token(&token);
        st.users.lock().await.drop_session(&h);
        let _ = st.store.delete_session(&h).await;
    }
    ([(axum::http::header::SET_COOKIE, session_cookie("", 0))], Json(serde_json::json!({ "ok": true })))
}

async fn me(State(st): State<AppState>, headers: HeaderMap) -> Result<Json<MeResp>, ApiErr> {
    let user = authed(&st, &headers).await?;
    let reg = st.users.lock().await;
    let email = reg.email_of(user).unwrap_or_default();
    Ok(Json(MeResp { user_id: user.0, email, verified: reg.is_verified(user) }))
}

/// Кто выполняет запрос: сначала сессия из cookie (ADR-018), затем — прежний
/// dev-токен `Bearer`. Легаси нужен, пока терминал переключает демо-пользователей
/// селектором; уйдёт вместе с его переездом на сессии.
async fn authed(st: &AppState, headers: &HeaderMap) -> Result<UserId, ApiErr> {
    if let Some(token) = cookie(headers, SESSION_COOKIE) {
        let users = st.users.lock().await;
        if let Some(user) = users.session(&auth::hash_session_token(&token), now_unix()) {
            return Ok(user);
        }
    }
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "not authenticated"))?;
    let users = st.users.lock().await;
    users.resolve(token).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "not authenticated"))
}

async fn create_user(State(st): State<AppState>, Json(req): Json<CreateUserReq>) -> Result<Json<CreateUserResp>, ApiErr> {
    let token = req.token.clone();
    let id = { st.users.lock().await.create(req.token) };
    if let Err(e) = st.store.save_user(id, &token).await {
        eprintln!("[gateway] не удалось сохранить пользователя: {e}");
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable"));
    }
    Ok(Json(CreateUserResp { user_id: id.0 }))
}

async fn deposit(State(st): State<AppState>, Json(req): Json<DepositReq>) -> Result<Json<BalanceResp>, ApiErr> {
    if req.amount <= 0 {
        return Err(err(StatusCode::BAD_REQUEST, "amount must be > 0"));
    }
    let mut o = st.orch.lock().await;
    o.deposit(UserId(req.user_id), AssetId(req.asset), Amount(req.amount));
    let b = o.balance(UserId(req.user_id), AssetId(req.asset));
    Ok(Json(BalanceResp { asset: req.asset, available: b.available.0, held: b.held.0 }))
}

/// Список инструментов с реальными котировками (Binance @ticker).
async fn list_instruments(State(st): State<AppState>) -> Json<Vec<InstrumentDto>> {
    let tickers = st.tickers.lock().await;
    let out = st
        .feed_instruments
        .iter()
        .map(|f| {
            let t = tickers.get(&f.id);
            InstrumentDto {
                id: f.id, symbol: f.symbol.clone(), base: f.base, quote: f.quote,
                price_decimals: f.price_decimals, qty_decimals: f.qty_decimals,
                last: t.map(|t| t.last), bid: t.map(|t| t.bid), ask: t.map(|t| t.ask),
                high: t.map(|t| t.high), change: t.map(|t| t.change).unwrap_or(0.0),
            }
        })
        .collect();
    Json(out)
}

/// Реальные свечи с Binance REST (кэш ~2с).
async fn get_candles(State(st): State<AppState>, Path(id): Path<u32>, Query(q): Query<CandleQuery>) -> Json<Vec<CandleDto>> {
    let tf = q.tf.unwrap_or(3600);
    let limit = q.limit.unwrap_or(300).min(1000);
    let Some(fi) = st.feed_instruments.iter().find(|f| f.id == id).cloned() else {
        return Json(vec![]);
    };
    let key = (id, tf);
    {
        let cache = st.klines_cache.lock().await;
        if let Some((t, v)) = cache.get(&key) {
            if t.elapsed() < Duration::from_secs(2) {
                return Json(tail_dto(v, limit));
            }
        }
    }
    match feed::fetch_klines(&fi.binance, feed::interval(tf), 500, fi.price_decimals, fi.qty_decimals).await {
        Ok(v) => {
            let out = tail_dto(&v, limit);
            st.klines_cache.lock().await.insert(key, (Instant::now(), v));
            Json(out)
        }
        Err(_) => Json(vec![]),
    }
}
fn tail_dto(v: &[feed::Kline], limit: usize) -> Vec<CandleDto> {
    v[v.len().saturating_sub(limit)..]
        .iter()
        .map(|k| CandleDto { time: k.time, open: k.open, high: k.high, low: k.low, close: k.close, volume: k.volume })
        .collect()
}

/// Стакан (order book) с Binance, кэш ~400мс.
async fn get_depth(State(st): State<AppState>, Path(id): Path<u32>, Query(q): Query<DepthQuery>) -> Json<DepthResp> {
    let Some(fi) = st.feed_instruments.iter().find(|f| f.id == id).cloned() else {
        return Json(DepthResp { price_decimals: 2, qty_decimals: 3, bids: vec![], asks: vec![] });
    };
    // Клиент просит глубину под свой шаг группировки: при грубом шаге 20 уровней
    // схлопываются в 2-3 строки и стакан выглядит полупустым. Binance принимает
    // только фиксированный набор значений — округляем вверх до ближайшего.
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
                return Json(DepthResp { price_decimals: fi.price_decimals, qty_decimals: fi.qty_decimals, bids: d.bids.clone(), asks: d.asks.clone() });
            }
        }
    }
    match feed::fetch_depth(&fi.binance, limit, fi.price_decimals, fi.qty_decimals).await {
        Ok(d) => {
            let resp = DepthResp { price_decimals: fi.price_decimals, qty_decimals: fi.qty_decimals, bids: d.bids.clone(), asks: d.asks.clone() };
            st.depth_cache.lock().await.insert((id, limit), (Instant::now(), d));
            Json(resp)
        }
        Err(_) => Json(DepthResp { price_decimals: fi.price_decimals, qty_decimals: fi.qty_decimals, bids: vec![], asks: vec![] }),
    }
}

/// Статистика актива: изменение (%) по таймфреймам, кэш ~3с.
async fn get_stats(State(st): State<AppState>, Path(id): Path<u32>) -> Json<Vec<StatDto>> {
    let Some(fi) = st.feed_instruments.iter().find(|f| f.id == id).cloned() else {
        return Json(vec![]);
    };
    {
        let cache = st.stats_cache.lock().await;
        if let Some((t, v)) = cache.get(&id) {
            if t.elapsed() < Duration::from_secs(3) {
                return Json(v.clone());
            }
        }
    }
    let futs = STAT_TFS.iter().map(|&tf| {
        let sym = fi.binance.clone();
        async move { StatDto { tf, change: feed::fetch_change(&sym, feed::interval(tf)).await.unwrap_or(0.0) } }
    });
    let out: Vec<StatDto> = futures_util::future::join_all(futs).await;
    st.stats_cache.lock().await.insert(id, (Instant::now(), out.clone()));
    Json(out)
}

async fn place_order(State(st): State<AppState>, headers: HeaderMap, Json(req): Json<PlaceOrderReq>) -> Result<Json<PlaceResp>, ApiErr> {
    let user = authed(&st, &headers).await?;
    let side = match req.side.as_str() {
        "buy" => Side::Buy,
        "sell" => Side::Sell,
        _ => return Err(err(StatusCode::BAD_REQUEST, "side must be buy|sell")),
    };
    if req.qty <= 0 {
        return Err(err(StatusCode::BAD_REQUEST, "qty must be > 0"));
    }
    let order_id = OrderId(st.order_seq.fetch_add(1, Ordering::SeqCst));
    let inst = InstrumentId(req.instrument);

    let result = {
        let mut o = st.orch.lock().await;
        match req.order_type.as_str() {
            "limit" => {
                let price = match req.price {
                    Some(p) => Price(p),
                    None => return Err(err(StatusCode::BAD_REQUEST, "limit order requires price")),
                };
                let tif = match req.tif.as_deref() {
                    Some("ioc") => TimeInForce::Ioc,
                    _ => TimeInForce::Gtc,
                };
                o.place_limit(user, inst, order_id, side, price, Qty(req.qty), tif)
            }
            "market" => o.place_market(user, inst, order_id, side, Qty(req.qty)),
            _ => return Err(err(StatusCode::BAD_REQUEST, "type must be limit|market")),
        }
    };

    match result {
        Ok(events) => {
            publish(&st, &events).await;
            Ok(Json(PlaceResp { order_id: order_id.0, events: events.iter().map(EventDto::from).collect() }))
        }
        Err(rej) => Err(reject_to_http(rej)),
    }
}

async fn cancel_order(State(st): State<AppState>, headers: HeaderMap, Path(id): Path<u64>, Query(q): Query<CancelQuery>) -> Result<Json<Vec<EventDto>>, ApiErr> {
    let user = authed(&st, &headers).await?;
    let result = {
        let mut o = st.orch.lock().await;
        o.cancel(user, InstrumentId(q.instrument), OrderId(id))
    };
    match result {
        Ok(events) => {
            publish(&st, &events).await;
            Ok(Json(events.iter().map(EventDto::from).collect()))
        }
        Err(rej) => Err(reject_to_http(rej)),
    }
}

async fn get_book(State(st): State<AppState>, Path(instrument): Path<u32>, Query(q): Query<BookQuery>) -> Result<Json<BookResp>, ApiErr> {
    let depth = q.depth.unwrap_or(20);
    let o = st.orch.lock().await;
    let snap = o.snapshot(InstrumentId(instrument), depth).ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown instrument"))?;
    let map = |lvls: Vec<exchange_core::Level>| lvls.into_iter().map(|l| LevelDto { price: l.price.0, qty: l.qty.0, orders: l.orders }).collect();
    Ok(Json(BookResp { bids: map(snap.bids), asks: map(snap.asks) }))
}

async fn get_balance(State(st): State<AppState>, headers: HeaderMap, Path(asset): Path<u32>) -> Result<Json<BalanceResp>, ApiErr> {
    let user = authed(&st, &headers).await?;
    let o = st.orch.lock().await;
    let b = o.balance(user, AssetId(asset));
    Ok(Json(BalanceResp { asset, available: b.available.0, held: b.held.0 }))
}

// ---- Бумажный брокер (ADR-014) --------------------------------------------

fn feed_by_id(st: &AppState, id: u32) -> Option<feed::FeedInstrument> {
    st.feed_instruments.iter().find(|f| f.id == id).cloned()
}

/// Открыть сделку по текущей рыночной цене (buy=ask, sell=bid).
async fn open_deal(State(st): State<AppState>, headers: HeaderMap, Json(req): Json<DealReq>) -> Result<Json<DealDto>, ApiErr> {
    let user = authed(&st, &headers).await?;
    let side = match req.side.as_str() {
        "buy" => PosSide::Long,
        "sell" => PosSide::Short,
        _ => return Err(err(StatusCode::BAD_REQUEST, "side must be buy|sell")),
    };
    if req.qty <= 0 {
        return Err(err(StatusCode::BAD_REQUEST, "qty must be > 0"));
    }
    let fi = feed_by_id(&st, req.instrument).ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown instrument"))?;
    let (entry, mark) = {
        let tickers = st.tickers.lock().await;
        let t = tickers.get(&req.instrument).ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "no price yet"))?;
        (if matches!(side, PosSide::Long) { t.ask } else { t.bid }, t.last)
    };
    // Ответ уходит только после успешной записи: клиент увидел 200 → данные на диске.
    let id = with_account(&st, user, |b| {
        b.open(user, req.instrument, side, req.qty, entry, fi.price_decimals, fi.qty_decimals, req.sl, req.tp)
            .map_err(|_| err(StatusCode::PAYMENT_REQUIRED, "insufficient_margin"))
    })
    .await?;
    Ok(Json(DealDto {
        id, instrument: req.instrument, symbol: fi.symbol, side: req.side, qty: req.qty, entry, mark, pnl: 0,
        sl: req.sl, tp: req.tp, price_decimals: fi.price_decimals, qty_decimals: fi.qty_decimals,
    }))
}

/// Разместить лимитный (отложенный) ордер.
async fn place_pending(State(st): State<AppState>, headers: HeaderMap, Json(req): Json<PendingReq>) -> Result<Json<PendingDto>, ApiErr> {
    let user = authed(&st, &headers).await?;
    let side = match req.side.as_str() {
        "buy" => PosSide::Long,
        "sell" => PosSide::Short,
        _ => return Err(err(StatusCode::BAD_REQUEST, "side must be buy|sell")),
    };
    if req.qty <= 0 || req.price <= 0 {
        return Err(err(StatusCode::BAD_REQUEST, "qty and price must be > 0"));
    }
    let fi = feed_by_id(&st, req.instrument).ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown instrument"))?;
    let id = with_account(&st, user, |b| {
        Ok(b.place_pending(user, req.instrument, side, req.qty, req.price, fi.price_decimals, fi.qty_decimals, req.sl, req.tp))
    })
    .await?;
    Ok(Json(PendingDto {
        id, instrument: req.instrument, symbol: fi.symbol, side: req.side, qty: req.qty, price: req.price,
        sl: req.sl, tp: req.tp, price_decimals: fi.price_decimals, qty_decimals: fi.qty_decimals,
    }))
}

async fn list_pending(State(st): State<AppState>, headers: HeaderMap) -> Result<Json<Vec<PendingDto>>, ApiErr> {
    let user = authed(&st, &headers).await?;
    let b = st.broker.lock().await;
    let out = b
        .pendings(user)
        .into_iter()
        .map(|p| PendingDto {
            id: p.id, instrument: p.instrument, symbol: feed_by_id(&st, p.instrument).map(|f| f.symbol).unwrap_or_default(),
            side: if matches!(p.side, PosSide::Long) { "buy" } else { "sell" }.to_string(),
            qty: p.qty, price: p.price, sl: p.sl, tp: p.tp, price_decimals: p.pd, qty_decimals: p.qd,
        })
        .collect();
    Ok(Json(out))
}

async fn cancel_pending(State(st): State<AppState>, headers: HeaderMap, Path(id): Path<u64>) -> Result<StatusCode, ApiErr> {
    let user = authed(&st, &headers).await?;
    with_account(&st, user, |b| {
        b.cancel_pending(user, id).map_err(|_| err(StatusCode::NOT_FOUND, "unknown pending"))
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_closed(State(st): State<AppState>, headers: HeaderMap) -> Result<Json<Vec<ClosedDto>>, ApiErr> {
    let user = authed(&st, &headers).await?;
    let b = st.broker.lock().await;
    let out = b
        .closed_deals(user)
        .into_iter()
        .rev()
        .map(|d| ClosedDto {
            instrument: d.instrument, symbol: feed_by_id(&st, d.instrument).map(|f| f.symbol).unwrap_or_default(),
            side: if matches!(d.side, PosSide::Long) { "buy" } else { "sell" }.to_string(),
            qty: d.qty, entry: d.entry, exit: d.exit, pnl: d.pnl, price_decimals: d.pd, qty_decimals: d.qd,
        })
        .collect();
    Ok(Json(out))
}

/// Фоновый монитор: триггеры лимитных ордеров и SL/TP по текущим ценам.
pub fn spawn_monitor(st: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
        loop {
            tick.tick().await;
            let m = marks(&st).await;
            if m.is_empty() {
                continue;
            }
            // Сохраняем только счета, которые реально изменились: обычный тик ничего
            // не триггерит и записи не делает (ADR-016). Слепки снимаем под тем же
            // локом, пишем — уже без него.
            let changed = { st.broker.lock().await.check(&m) };
            for user in changed {
                // Под локом счёта переснимаем состояние: если параллельно прошла
                // операция пользователя, запишем актуальное, а не устаревшее.
                let lock = account_lock(&st, user).await;
                let _guard = lock.lock().await;
                let snap = { st.broker.lock().await.snapshot(user) };
                if let Err(e) = st.store.save_account(user, &snap).await {
                    // Откатывать нечего: триггер уже применён к рабочему состоянию.
                    // Потеря ограничена последним тиком — после перезапуска несработавшие
                    // триггеры сработают снова по актуальным ценам (ADR-016).
                    eprintln!("[monitor] не удалось сохранить счёт {}: {e}", user.0);
                }
            }
        }
    });
}

/// Список открытых позиций с live P&L.
async fn list_deals(State(st): State<AppState>, headers: HeaderMap) -> Result<Json<Vec<DealDto>>, ApiErr> {
    let user = authed(&st, &headers).await?;
    let m = marks(&st).await;
    let b = st.broker.lock().await;
    let out = b
        .positions(user)
        .into_iter()
        .map(|p| {
            let mark = *m.get(&p.instrument).unwrap_or(&p.entry);
            let symbol = feed_by_id(&st, p.instrument).map(|f| f.symbol).unwrap_or_default();
            DealDto {
                id: p.id, instrument: p.instrument, symbol,
                side: if matches!(p.side, PosSide::Long) { "buy" } else { "sell" }.to_string(),
                qty: p.qty, entry: p.entry, mark, pnl: p.unrealized(mark),
                sl: p.sl, tp: p.tp, price_decimals: p.pd, qty_decimals: p.qd,
            }
        })
        .collect();
    Ok(Json(out))
}

/// Закрыть позицию по текущей цене (last).
async fn close_deal(State(st): State<AppState>, headers: HeaderMap, Path(id): Path<u64>) -> Result<Json<serde_json::Value>, ApiErr> {
    let user = authed(&st, &headers).await?;
    let m = marks(&st).await;
    let (pnl, mark) = with_account(&st, user, |b| {
        let inst = b
            .positions(user)
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.instrument)
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown position"))?;
        let mark = *m.get(&inst).unwrap_or(&0);
        let pnl = b.close(user, id, mark).map_err(|_| err(StatusCode::NOT_FOUND, "unknown position"))?;
        Ok((pnl, mark))
    })
    .await?;
    Ok(Json(serde_json::json!({ "pnl": pnl, "mark": mark })))
}

/// Сводка счёта: баланс/equity/маржа/free/open P&L (в центах).
async fn account(State(st): State<AppState>, headers: HeaderMap) -> Result<Json<AccountDto>, ApiErr> {
    let user = authed(&st, &headers).await?;
    let m = marks(&st).await;
    let b = st.broker.lock().await;
    let balance = b.balance(user);
    let open_pnl = b.open_pnl(user, &m);
    let used = b.used_margin(user);
    let equity = balance + open_pnl;
    Ok(Json(AccountDto { balance, equity, used_margin: used, free_margin: equity - used, open_pnl }))
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

// ============================ Демо-сид (для тестов матчинга) ================

/// Минимальный сид: инструмент 1 (BTC-USDT) в оркестраторе + alice/bob. Нужен тестам матчинга и
/// как фундамент под будущее broker-исполнение. Реальные данные терминала идут из [`feed`].
pub async fn seed_demo(st: &AppState) {
    let (alice, bob) = {
        let mut users = st.users.lock().await;
        (users.get_or_create("alice-token".into()), users.get_or_create("bob-token".into()))
    };
    // Новых демо-пользователей фиксируем в хранилище, иначе их счета не с чем связать.
    for (id, token, created) in [(alice.0, "alice-token", alice.1), (bob.0, "bob-token", bob.1)] {
        if created {
            if let Err(e) = st.store.save_user(id, token).await {
                eprintln!("[gateway] демо-пользователь не сохранён: {e}");
            }
        }
    }
    let (alice, bob) = (alice.0, bob.0);
    let mut o = st.orch.lock().await;
    o.register_instrument(Instrument {
        id: InstrumentId(1),
        symbol: "BTC-USDT".into(),
        base: AssetId(1),
        quote: AssetId(2),
        price_decimals: 2,
        qty_decimals: 3,
        tick_size: Price(1),
        lot_size: Qty(1),
        min_qty: Qty(1),
    });
    for u in [alice, bob] {
        o.deposit(u, AssetId(1), Amount(1_000_000));
        o.deposit(u, AssetId(2), Amount(100_000_000));
    }
}
