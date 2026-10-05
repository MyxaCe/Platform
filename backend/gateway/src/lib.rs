//! # gateway — шлюз терминала
//!
//! REST + WS для торгового терминала. Рыночные данные — **реальные с Binance** (ADR-013):
//! `/instruments`, `/candles`, `/depth`, `/stats` из [`feed`]; лента и live-цена — через WS `/stream`.
//! Торговля — **бумажный брокер** (ADR-014): позиции, маржа, P&L, SL/TP, лимитные ордера.
//!
//! Терминал автономный: без логина, один аккаунт [`DEFAULT_USER`]. Как его авторизовать из внешних
//! сайтов/CMS — отдельное решение (пивот 2026-07-25, см. STATUS). Доменные типы наружу не протекают.

pub mod feed;
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

use broker::{Broker, PosSide};
use persistence::{NoopStore, Persistence};

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
    /// Разрешённые инструменты по сайтам (регуляторика, ADR-023 Т3).
    ///
    /// Источник по контракту — `GET {CMS}/v1/cms/sites/{slug}`, поле `instruments`
    /// с `terminalInstrumentId`, сервер-сервер под `X-API-Key`. Ключа контура у нас
    /// пока нет, поэтому источник временно читается из env `TENANT_INSTRUMENTS`
    /// (см. `parse_tenant_instruments`). Шов один и тот же: когда ключ появится,
    /// меняется только наполнение карты.
    ///
    /// `None` — источник не сконфигурирован вовсе. Это **не** «всё разрешено»:
    /// в тенантном режиме отсутствие списка закрывает торговлю (fail-closed).
    tenant_instruments: Arc<Option<HashMap<String, Vec<u32>>>>,
    // SSO платформы (ADR-023, Т2): вход по handoff-JWT, сессии терминала.
    sso: Arc<sso::Sso>,
    /// Требовать ли сессию на торговых ручках. Выкл (dev/standalone) → счёт по умолчанию.
    sso_enabled: bool,
}

pub fn build_state() -> AppState {
    build_state_with(Arc::new(NoopStore))
}

/// Собрать состояние с конкретным хранилищем. `NoopStore` = всё в памяти (тесты, запуск без БД).
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
    AppState {
        events_tx,
        feed_instruments: Arc::new(feed::instruments()),
        tickers: Arc::new(Mutex::new(HashMap::new())),
        feed_last_msg: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        klines_cache: Arc::new(Mutex::new(HashMap::new())),
        depth_cache: Arc::new(Mutex::new(HashMap::new())),
        stats_cache: Arc::new(Mutex::new(HashMap::new())),
        tenant_instruments: Arc::new(parse_tenant_instruments(std::env::var("TENANT_INSTRUMENTS").ok().as_deref())),
        broker: Arc::new(Mutex::new(Broker::new(start_cents, 1))), // старт из env, leverage 1
        store,
        acct_locks: Arc::new(Mutex::new(HashMap::new())),
        quarantined: Arc::new(Mutex::new(std::collections::HashSet::new())),
        sso,
        sso_enabled,
    }
}

/// Поднять счета из хранилища. Вызывается один раз на старте, до приёма запросов.
pub async fn restore_state(st: &AppState) -> Result<(), persistence::StoreError> {
    st.store.init().await?;
    let loaded = st.store.load_all().await?;
    let accounts_n = loaded.accounts.len();
    {
        let mut b = st.broker.lock().await;
        for (id, snap) in loaded.accounts {
            b.restore(id, snap);
        }
    }
    // Красная линия №5. Счета, поднятые с нарушенными инвариантами, уходят в
    // карантин: читать можно, менять нельзя. Сервис при этом стартует — стоп
    // сервиса не применяется нигде (КОНТРАКТЫ §4в), потому что одна испорченная
    // строка не должна лишать обслуживания всех остальных.
    {
        let b = st.broker.lock().await;
        let mut q = st.quarantined.lock().await;
        for user in b.users() {
            let violations = broker::check_snapshot(&b.snapshot(user), b.leverage());
            if !violations.is_empty() {
                q.insert(user);
                for v in &violations {
                    eprintln!("[gateway] ИНВАРИАНТ НАРУШЕН, счёт {} в карантине: {v}", user.0);
                }
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

/// Лок конкретного счёта (создаётся лениво).
async fn account_lock(st: &AppState, user: UserId) -> Arc<Mutex<()>> {
    st.acct_locks.lock().await.entry(user).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
}

/// Мутация счёта с сохранением (ADR-016): лок счёта на всё «мутация → слепок → запись»;
/// `fsync`/сеть — уже без лока брокера; при отказе хранилища счёт откатывается и отдаётся `503`.
async fn with_account<T>(st: &AppState, user: UserId, f: impl FnOnce(&mut Broker) -> Result<T, ApiErr>) -> Result<T, ApiErr> {
    // Счёт в карантине не мутируем: его состояние уже неверно, и любая операция
    // поверх него закрепила бы порчу. Читать при этом можно.
    if st.quarantined.lock().await.contains(&user) {
        return Err(err(StatusCode::LOCKED, "account_quarantined"));
    }

    let lock = account_lock(st, user).await;
    let _guard = lock.lock().await;

    let (out, before, snap) = {
        let mut b = st.broker.lock().await;
        let before = b.snapshot(user);
        let out = f(&mut b)?;
        let snap = b.snapshot(user);
        (out, before, snap)
    };

    // Красная линия №5, проверка ДО записи: если мутация сделала бы состояние
    // неверным, операция отклоняется и изменение не фиксируется. Граница отказа
    // проходит по счёту — соседи не затронуты, сервис не останавливается.
    // Код отдельный: это не деловой отказ вроде «недостаточно средств», и путать
    // их нельзя (КОНТРАКТЫ §4в).
    let violations = {
        let b = st.broker.lock().await;
        broker::check_snapshot(&snap, b.leverage())
    };
    if !violations.is_empty() {
        st.broker.lock().await.restore(user, before);
        for v in &violations {
            eprintln!("[gateway] ИНВАРИАНТ НАРУШЕН операцией, счёт {} откачен: {v}", user.0);
        }
        return Err(err(StatusCode::UNPROCESSABLE_ENTITY, "invariant_violated"));
    }

    if let Err(e) = st.store.save_account(user, &snap).await {
        st.broker.lock().await.restore(user, before);
        eprintln!("[gateway] запись состояния не удалась, счёт откачен: {e}");
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable"));
    }
    Ok(out)
}

impl AppState {
    /// Отметить, что от Binance пришло опознанное сообщение (см. `feed_last_msg`).
    pub(crate) fn mark_feed_alive(&self) {
        self.feed_last_msg.store(now_secs(), std::sync::atomic::Ordering::Relaxed);
    }
}

/// Разобрать временный источник allow-list: `site:1,2,3;other:4,5`.
///
/// `None` — переменная не задана (источника нет). Пустой список у сайта — валидное
/// значение и означает «сайту не разрешено ничего», а не «фильтра нет».
fn parse_tenant_instruments(raw: Option<&str>) -> Option<HashMap<String, Vec<u32>>> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let mut out: HashMap<String, Vec<u32>> = HashMap::new();
    for site_part in raw.split(';').filter(|p| !p.trim().is_empty()) {
        let (site, ids) = site_part.split_once(':').unwrap_or((site_part, ""));
        let ids = ids
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse::<u32>().ok())
            .collect();
        out.insert(site.trim().to_string(), ids);
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
fn ensure_can_open(st: &AppState, who: &Principal, instrument: u32) -> Result<(), ApiErr> {
    let Some(tenant) = who.tenant.as_deref() else {
        return Ok(()); // standalone: сайта нет, ограничивать нечем и не от чьего имени
    };
    let Some(map) = st.tenant_instruments.as_ref() else {
        eprintln!("[gateway] {tenant}: источник allow-list не сконфигурирован — торговля закрыта");
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "tenant_instruments_unavailable"));
    };
    let Some(allowed) = map.get(tenant) else {
        eprintln!("[gateway] {tenant}: сайта нет в allow-list — торговля закрыта");
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "tenant_instruments_unavailable"));
    };
    if allowed.contains(&instrument) {
        Ok(())
    } else {
        Err(err(StatusCode::FORBIDDEN, "instrument_not_allowed"))
    }
}

/// Текущие марк-цены (инструмент → last) из фида — для P&L/equity брокера.
async fn marks(st: &AppState) -> HashMap<u32, i64> {
    st.tickers.lock().await.iter().map(|(id, t)| (*id, t.last)).collect()
}

fn feed_by_id(st: &AppState, id: u32) -> Option<feed::FeedInstrument> {
    st.feed_instruments.iter().find(|f| f.id == id).cloned()
}

/// `Authorization: Bearer <token>` → сам токен.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get("authorization")?.to_str().ok()?.strip_prefix("Bearer ")
}

/// Кто выполняет запрос (ADR-023, Т2). SSO включён — из bearer-сессии, иначе `401`;
/// SSO выключен (dev/standalone) — счёт по умолчанию и никакого сайта.
async fn require_principal(st: &AppState, headers: &HeaderMap) -> Result<Principal, ApiErr> {
    if !st.sso_enabled {
        return Ok(Principal { user: DEFAULT_USER, tenant: None });
    }
    let token = bearer(headers).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "missing session"))?;
    let (user, tenant) = st
        .sso
        .resolve(token)
        .await
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "invalid session"))?;
    Ok(Principal { user, tenant: Some(tenant) })
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
    (code, Json(ErrResp { error: msg.to_string() }))
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
    })
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Список инструментов с реальными котировками (Binance @ticker).
///
/// Витрина: фильтруется по `?site=`, если он задан. Это удобство, а не защита —
/// защита стоит на пути открытия сделки (`ensure_can_open`), потому что `?site=`
/// из браузера подставит кто угодно. Отказ отдаётся кодом, а не пустым списком:
/// пустой список читается как «сайту нечем торговать», что неотличимо от
/// «конфиг сайта не доехал» (BUG-011).
async fn list_instruments(State(st): State<AppState>, Query(q): Query<SiteQuery>) -> Result<Json<Vec<InstrumentDto>>, ApiErr> {
    let allowed: Option<Vec<u32>> = match q.site.as_deref().filter(|s| !s.is_empty()) {
        None => None,
        Some(site) => match st.tenant_instruments.as_ref().as_ref().and_then(|m| m.get(site)) {
            Some(list) => Some(list.clone()),
            None => {
                eprintln!("[gateway] {site}: нет allow-list — список инструментов не отдаём");
                return Err(err(StatusCode::SERVICE_UNAVAILABLE, "tenant_instruments_unavailable"));
            }
        },
    };
    let tickers = st.tickers.lock().await;
    let out = st
        .feed_instruments
        .iter()
        .filter(|f| allowed.as_ref().is_none_or(|a| a.contains(&f.id)))
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
    Ok(Json(out))
}

/// Реальные свечи с Binance REST (кэш ~2с).
///
/// Отказ отдаётся кодом, а не пустым массивом: `[]` читается как «рынок пуст» и
/// стирает график, хотя означал «мы не смогли получить данные» (BUG-011).
async fn get_candles(State(st): State<AppState>, Path(id): Path<u32>, Query(q): Query<CandleQuery>) -> Result<Json<Vec<CandleDto>>, ApiErr> {
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
    match feed::fetch_klines(&fi.binance, feed::interval(tf), 500, fi.price_decimals, fi.qty_decimals).await {
        Ok(v) => {
            let out = tail_dto(&v, limit);
            st.klines_cache.lock().await.insert(key, (Instant::now(), v));
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
        .map(|k| CandleDto { time: k.time, open: k.open, high: k.high, low: k.low, close: k.close, volume: k.volume })
        .collect()
}

/// Стакан (order book) с Binance, кэш ~400мс.
///
/// Как и свечи: отказ — кодом, а не пустым стаканом. Пустой стакан означает «нет
/// заявок» — совсем другое утверждение, чем «мы не дошли до биржи» (BUG-011).
async fn get_depth(State(st): State<AppState>, Path(id): Path<u32>, Query(q): Query<DepthQuery>) -> Result<Json<DepthResp>, ApiErr> {
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
                return Ok(Json(DepthResp { price_decimals: fi.price_decimals, qty_decimals: fi.qty_decimals, bids: d.bids.clone(), asks: d.asks.clone() }));
            }
        }
    }
    match feed::fetch_depth(&fi.binance, limit, fi.price_decimals, fi.qty_decimals).await {
        Ok(d) => {
            let resp = DepthResp { price_decimals: fi.price_decimals, qty_decimals: fi.qty_decimals, bids: d.bids.clone(), asks: d.asks.clone() };
            st.depth_cache.lock().await.insert((id, limit), (Instant::now(), d));
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
async fn get_stats(State(st): State<AppState>, Path(id): Path<u32>) -> Result<Json<Vec<StatDto>>, ApiErr> {
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
                Ok(change) => StatDto { tf, change: Some(change) },
                Err(e) => {
                    eprintln!("[gateway] статистика {label} tf={tf} недоступна: {e}");
                    StatDto { tf, change: None }
                }
            }
        }
    });
    let out: Vec<StatDto> = futures_util::future::join_all(futs).await;
    st.stats_cache.lock().await.insert(id, (Instant::now(), out.clone()));
    Ok(Json(out))
}

/// Открыть сделку по текущей рыночной цене (buy=ask, sell=bid).
async fn open_deal(State(st): State<AppState>, headers: HeaderMap, Json(req): Json<DealReq>) -> Result<Json<DealDto>, ApiErr> {
    let who = require_principal(&st, &headers).await?;
    ensure_can_open(&st, &who, req.instrument)?;
    let user = who.user;
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
    // Лимитный ордер — такой же путь открытия позиции, только отложенный во времени.
    // Проверять только рыночный вход значило бы оставить дверь рядом с турникетом.
    let who = require_principal(&st, &headers).await?;
    ensure_can_open(&st, &who, req.instrument)?;
    let user = who.user;
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
    let user = require_user(&st, &headers).await?;
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
    let user = require_user(&st, &headers).await?;
    with_account(&st, user, |b| {
        b.cancel_pending(user, id).map_err(|_| err(StatusCode::NOT_FOUND, "unknown pending"))
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_closed(State(st): State<AppState>, headers: HeaderMap) -> Result<Json<Vec<ClosedDto>>, ApiErr> {
    let user = require_user(&st, &headers).await?;
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

/// Список открытых позиций с live P&L.
async fn list_deals(State(st): State<AppState>, headers: HeaderMap) -> Result<Json<Vec<DealDto>>, ApiErr> {
    let user = require_user(&st, &headers).await?;
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
///
/// Allow-list здесь сознательно **не** проверяется: он ограничивает набор
/// разрешённого, а закрытие позицию сокращает. Если инструмент запретили, пока
/// позиция открыта, запереть человека в ней было бы хуже запрета — он не смог бы
/// выйти из того, что ему больше не разрешено держать. То же для отмены ордера.
async fn close_deal(State(st): State<AppState>, headers: HeaderMap, Path(id): Path<u64>) -> Result<Json<serde_json::Value>, ApiErr> {
    let user = require_user(&st, &headers).await?;
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
    Ok(Json(AccountDto { model, balance, equity, used_margin: used, free_margin: equity - used, open_pnl }))
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
async fn session_create(State(st): State<AppState>, Json(req): Json<SessionReq>) -> Result<Json<SessionResp>, ApiErr> {
    let claims = st.sso.validate(&req.token, &req.site).await.map_err(sso_err)?;
    let user = st
        .store
        .resolve_identity(&claims.tenant, &claims.sub)
        .await
        .map_err(|_| err(StatusCode::SERVICE_UNAVAILABLE, "identity store unavailable"))?;
    let expires_in = st.sso.mint(claims.jti.clone(), user, claims.tenant.clone()).await;
    Ok(Json(SessionResp { token: claims.jti, expires_in }))
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
            let m = marks(&st).await;
            if m.is_empty() {
                continue;
            }
            let changed = { st.broker.lock().await.check(&m) };
            for user in changed {
                let lock = account_lock(&st, user).await;
                let _guard = lock.lock().await;
                let snap = { st.broker.lock().await.snapshot(user) };
                // Триггеры SL/TP и лимитных ордеров меняют счёт так же, как ручные
                // операции, поэтому и проверяются так же. Иначе инвариант защищал бы
                // только тот путь, о котором мы помним.
                let violations = {
                    let b = st.broker.lock().await;
                    broker::check_snapshot(&snap, b.leverage())
                };
                if !violations.is_empty() {
                    st.quarantined.lock().await.insert(user);
                    for v in &violations {
                        eprintln!("[monitor] ИНВАРИАНТ НАРУШЕН триггером, счёт {} в карантине: {v}", user.0);
                    }
                    continue; // испорченное не записываем и не чиним
                }
                if let Err(e) = st.store.save_account(user, &snap).await {
                    eprintln!("[monitor] не удалось сохранить счёт {}: {e}", user.0);
                }
            }
        }
    });
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
    async fn tenant_app(map: Option<HashMap<String, Vec<u32>>>) -> (Router, String) {
        let mut st = build_state();
        st.sso = Arc::new(sso::Sso::new("http://jwks.invalid/jwks".into()));
        st.sso_enabled = true;
        st.tenant_instruments = Arc::new(map);
        let token = "session-token".to_string();
        st.sso.mint(token.clone(), DEFAULT_USER, SITE.to_string()).await;
        (router(st), token)
    }

    fn allow(site: &str, ids: &[u32]) -> HashMap<String, Vec<u32>> {
        let mut m = HashMap::new();
        m.insert(site.to_string(), ids.to_vec());
        m
    }

    async fn post(app: &Router, uri: &str, token: Option<&str>, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let mut b = Request::builder().method("POST").uri(uri).header("content-type", "application/json");
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        let req = b.body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap();
        read(app.clone().oneshot(req).await.unwrap()).await
    }

    async fn get(app: &Router, uri: &str, token: Option<&str>) -> (StatusCode, serde_json::Value) {
        let mut b = Request::builder().method("GET").uri(uri);
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        read(app.clone().oneshot(b.body(Body::empty()).unwrap()).await.unwrap()).await
    }

    async fn read(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null) };
        (status, v)
    }

    fn deal(instrument: u32) -> serde_json::Value {
        serde_json::json!({"instrument": instrument, "side": "buy", "qty": 100})
    }
    fn pending(instrument: u32) -> serde_json::Value {
        serde_json::json!({"instrument": instrument, "side": "buy", "qty": 100, "price": 1000})
    }

    // ---- Разбор источника ---------------------------------------------------

    #[test]
    fn parses_source() {
        let m = parse_tenant_instruments(Some("apex-ru:1,2,3;other:4")).expect("источник задан");
        assert_eq!(m.get("apex-ru"), Some(&vec![1, 2, 3]));
        assert_eq!(m.get("other"), Some(&vec![4]));
    }

    /// Пустой список у сайта — валидное значение «сайту не разрешено ничего»,
    /// а не «фильтра нет». Разница регуляторная, поэтому проверяем отдельно.
    #[test]
    fn empty_list_is_a_value_not_absence() {
        let m = parse_tenant_instruments(Some("apex-ru:")).expect("источник задан");
        assert_eq!(m.get("apex-ru"), Some(&vec![]));
    }

    #[test]
    fn absent_source_is_none() {
        assert!(parse_tenant_instruments(None).is_none());
        assert!(parse_tenant_instruments(Some("   ")).is_none());
    }

    // ---- Граница: открытие позиции -----------------------------------------

    /// Рыночный вход по инструменту вне списка сайта — 403.
    #[tokio::test]
    async fn open_deal_rejects_instrument_outside_allow_list() {
        let (app, tok) = tenant_app(Some(allow(SITE, &[1]))).await;
        let (status, body) = post(&app, "/deals", Some(&tok), deal(2)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "инструмент вне allow-list не должен торговаться");
        assert_eq!(body["error"], "instrument_not_allowed");
    }

    /// Лимитный ордер — тот же путь открытия позиции. Дверь рядом с турникетом.
    #[tokio::test]
    async fn pending_rejects_instrument_outside_allow_list() {
        let (app, tok) = tenant_app(Some(allow(SITE, &[1]))).await;
        let (status, body) = post(&app, "/pending", Some(&tok), pending(2)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "отложенный ордер обязан проверяться так же, как рыночный");
        assert_eq!(body["error"], "instrument_not_allowed");
    }

    /// Разрешённый инструмент проходит границу (лимитный ордер не требует живой цены).
    #[tokio::test]
    async fn allowed_instrument_passes() {
        let (app, tok) = tenant_app(Some(allow(SITE, &[1]))).await;
        let (status, _) = post(&app, "/pending", Some(&tok), pending(1)).await;
        assert_eq!(status, StatusCode::OK, "разрешённый инструмент обязан торговаться");
    }

    /// Пустой список запрещает всё — включая инструмент №1.
    #[tokio::test]
    async fn empty_allow_list_denies_everything() {
        let (app, tok) = tenant_app(Some(allow(SITE, &[]))).await;
        let (status, _) = post(&app, "/deals", Some(&tok), deal(1)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    // ---- Fail-closed --------------------------------------------------------

    /// Источник не сконфигурирован → торговля закрыта (КОНТРАКТЫ §4б).
    /// Отказ инфраструктуры не должен превращаться в разрешение.
    #[tokio::test]
    async fn missing_source_closes_trading() {
        let (app, tok) = tenant_app(None).await;
        for (uri, body) in [("/deals", deal(1)), ("/pending", pending(1))] {
            let (status, b) = post(&app, uri, Some(&tok), body).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{uri}: без конфига торговли быть не должно");
            assert_eq!(b["error"], "tenant_instruments_unavailable", "{uri}: причина обязана отличаться от «не разрешено»");
        }
    }

    /// Сайта нет в реестре → тоже закрыто, и причина та же: списка нет.
    #[tokio::test]
    async fn unknown_site_closes_trading() {
        let (app, tok) = tenant_app(Some(allow("some-other-site", &[1, 2]))).await;
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
        let (app, tok) = tenant_app(Some(allow(SITE, &[]))).await;
        let (status, _) = post(&app, "/deals/1/close", Some(&tok), serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "закрытие не должно упираться в allow-list");
    }

    /// Отмена отложенного ордера — тоже сокращение риска, тоже не ограничивается.
    /// Пара к предыдущему тесту: если «для единообразия» границу поставят на все
    /// торговые ручки подряд, покраснеет один из этих двух.
    #[tokio::test]
    async fn cancelling_is_never_blocked_by_allow_list() {
        let (app, tok) = tenant_app(Some(allow(SITE, &[]))).await;
        let req = Request::builder()
            .method("DELETE")
            .uri("/pending/1")
            .header("authorization", format!("Bearer {tok}"))
            .body(Body::empty())
            .unwrap();
        let (status, _) = read(app.oneshot(req).await.unwrap()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "отмена ордера не должна упираться в allow-list");
    }

    /// SL/TP — атрибуты позиции (`Position { sl, tp }`), а не отдельные ордера:
    /// закрытие по ним идёт фоновым монитором, мимо роутера и мимо границы. Тест
    /// фиксирует именно это: позиция с SL/TP на разрешённом инструменте заводится
    /// одним запросом, отдельной «ручки закрытия» для неё не требуется.
    #[tokio::test]
    async fn sl_tp_travel_with_the_position_not_as_orders() {
        let (app, tok) = tenant_app(Some(allow(SITE, &[1]))).await;
        let body = serde_json::json!({"instrument": 1, "side": "buy", "qty": 100, "price": 1000, "sl": 900, "tp": 1200});
        let (status, v) = post(&app, "/pending", Some(&tok), body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["sl"], 900);
        assert_eq!(v["tp"], 1200);
    }

    /// Чтение позиций и счёта сайтом не ограничивается.
    #[tokio::test]
    async fn reading_account_is_not_blocked() {
        let (app, tok) = tenant_app(Some(allow(SITE, &[]))).await;
        let (status, _) = get(&app, "/account", Some(&tok)).await;
        assert_eq!(status, StatusCode::OK);
    }

    // ---- Витрина ------------------------------------------------------------

    #[tokio::test]
    async fn instruments_listing_filtered_by_site() {
        let (app, _) = tenant_app(Some(allow(SITE, &[2, 3]))).await;
        let (status, body) = get(&app, "/instruments?site=apex-ru", None).await;
        assert_eq!(status, StatusCode::OK);
        let ids: Vec<u64> = body.as_array().unwrap().iter().map(|i| i["id"].as_u64().unwrap()).collect();
        assert_eq!(ids, vec![2, 3], "витрина обязана показывать только разрешённое");
    }

    /// Без конфига витрина отказывает кодом, а не отдаёт пустой список: пустой
    /// список читается как «сайту нечем торговать» (BUG-011).
    #[tokio::test]
    async fn instruments_listing_fails_loudly_without_config() {
        let (app, _) = tenant_app(None).await;
        let (status, b) = get(&app, "/instruments?site=apex-ru", None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(b["error"], "tenant_instruments_unavailable");
    }

    /// Без `?site=` витрина отдаёт всё: это standalone-режим, юрисдикции нет.
    #[tokio::test]
    async fn instruments_listing_unfiltered_without_site() {
        let (app, _) = tenant_app(Some(allow(SITE, &[1]))).await;
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
                id: 1, instrument: 1, side: PosSide::Long,
                qty: 0, entry: 5000, pd: 2, qd: 3, margin: 5_000, sl: None, tp: None,
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
            Ok(persistence::LoadedState { accounts: vec![(DEFAULT_USER, corrupt_snapshot())] })
        }
        async fn save_account(&self, _u: UserId, _s: &broker::AccountSnapshot) -> Result<(), persistence::StoreError> {
            Ok(())
        }
    }

    async fn send(app: &Router, method: &str, uri: &str, body: Option<serde_json::Value>) -> (StatusCode, serde_json::Value) {
        let b = Request::builder().method(method).uri(uri);
        let req = match body {
            Some(v) => b.header("content-type", "application/json").body(Body::from(serde_json::to_vec(&v).unwrap())).unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = if bytes.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null) };
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
        st.broker.lock().await.restore(DEFAULT_USER, corrupt_snapshot());
        let app = router(st.clone());

        let (status, body) = send(&app, "POST", "/pending", Some(pending_body())).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "операция, оставляющая счёт неверным, обязана быть отклонена");
        assert_eq!(body["error"], "invariant_violated", "отказ обязан отличаться от делового");

        // Изменение не зафиксировано: ордер не добавился.
        let (_, list) = send(&app, "GET", "/pending", None).await;
        assert_eq!(list.as_array().map(|a| a.len()), Some(0), "изменение не должно фиксироваться");
    }

    /// Отказ по инварианту отличим от делового отказа. Их нельзя путать: первый
    /// означает «состояние сломано», второй — «денег не хватило».
    #[tokio::test]
    async fn invariant_rejection_differs_from_business_rejection() {
        let st = build_state();
        st.broker.lock().await.restore(DEFAULT_USER, corrupt_snapshot());
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
        restore_state(&st).await.expect("сервис обязан стартовать, а не падать");
        assert!(st.quarantined.lock().await.contains(&DEFAULT_USER), "счёт обязан уйти в карантин");
    }

    /// В карантине: чтение разрешено, мутации отклоняются.
    #[tokio::test]
    async fn quarantined_account_reads_but_does_not_mutate() {
        let st = build_state_with(Arc::new(CorruptStore));
        restore_state(&st).await.unwrap();
        let app = router(st);

        let (read_status, _) = send(&app, "GET", "/account", None).await;
        assert_eq!(read_status, StatusCode::OK, "чтение в карантине обязано работать");

        let (write_status, body) = send(&app, "POST", "/pending", Some(pending_body())).await;
        assert_eq!(write_status, StatusCode::LOCKED, "мутации в карантине обязаны отклоняться");
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
        assert_eq!(snap.positions[0].qty, 0, "нулевой объём обязан сохраниться как улика");
        assert!(!broker::check_snapshot(&snap, 1).is_empty(), "состояние обязано остаться помеченным как неверное");
    }

    /// Здоровое состояние в карантин не уходит — проверка не срабатывает вхолостую.
    #[tokio::test]
    async fn healthy_account_is_not_quarantined() {
        let st = build_state();
        restore_state(&st).await.unwrap();
        assert!(st.quarantined.lock().await.is_empty());
        let app = router(st);
        let (status, _) = send(&app, "POST", "/pending", Some(pending_body())).await;
        assert_eq!(status, StatusCode::OK, "нормальная торговля не должна страдать от проверок");
    }
}
