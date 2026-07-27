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
    klines_cache: Arc<Mutex<KlineCache>>,
    depth_cache: Arc<Mutex<DepthCache>>,
    stats_cache: Arc<Mutex<StatsCache>>,
    // Бумажный брокер (ADR-014):
    broker: Arc<Mutex<Broker>>,
    // Durable-хранилище счетов (ADR-016).
    store: Arc<dyn Persistence>,
    /// Лок на счёт: сериализует «мутация → слепок → запись» по одному счёту.
    acct_locks: Arc<Mutex<HashMap<UserId, Arc<Mutex<()>>>>>,
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
        klines_cache: Arc::new(Mutex::new(HashMap::new())),
        depth_cache: Arc::new(Mutex::new(HashMap::new())),
        stats_cache: Arc::new(Mutex::new(HashMap::new())),
        broker: Arc::new(Mutex::new(Broker::new(start_cents, 1))), // старт из env, leverage 1
        store,
        acct_locks: Arc::new(Mutex::new(HashMap::new())),
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
    // Красная линия №5: инварианты денег проверяются в рантайме, нарушение = стоп.
    let broken = st.store.check_invariants().await?;
    if !broken.is_empty() {
        return Err(persistence::StoreError(format!("нарушены инварианты: {}", broken.join("; "))));
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
    let lock = account_lock(st, user).await;
    let _guard = lock.lock().await;

    let (out, before, snap) = {
        let mut b = st.broker.lock().await;
        let before = b.snapshot(user);
        let out = f(&mut b)?;
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

fn feed_by_id(st: &AppState, id: u32) -> Option<feed::FeedInstrument> {
    st.feed_instruments.iter().find(|f| f.id == id).cloned()
}

/// `Authorization: Bearer <token>` → сам токен.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get("authorization")?.to_str().ok()?.strip_prefix("Bearer ")
}

/// Пользователь запроса (ADR-023, Т2). SSO включён — из bearer-сессии, иначе `401`;
/// SSO выключен (dev/standalone) — счёт по умолчанию.
async fn require_user(st: &AppState, headers: &HeaderMap) -> Result<UserId, ApiErr> {
    if !st.sso_enabled {
        return Ok(DEFAULT_USER);
    }
    let token = bearer(headers).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "missing session"))?;
    st.sso.resolve(token).await.ok_or_else(|| err(StatusCode::UNAUTHORIZED, "invalid session"))
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

// ============================ Хендлеры =====================================

async fn health() -> &'static str {
    "ok"
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
    let Some(fi) = feed_by_id(&st, id) else {
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
    let Some(fi) = feed_by_id(&st, id) else {
        return Json(DepthResp { price_decimals: 2, qty_decimals: 3, bids: vec![], asks: vec![] });
    };
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
    let Some(fi) = feed_by_id(&st, id) else {
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

/// Открыть сделку по текущей рыночной цене (buy=ask, sell=bid).
async fn open_deal(State(st): State<AppState>, headers: HeaderMap, Json(req): Json<DealReq>) -> Result<Json<DealDto>, ApiErr> {
    let user = require_user(&st, &headers).await?;
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
    let user = require_user(&st, &headers).await?;
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
    Ok(Json(AccountDto { balance, equity, used_margin: used, free_margin: equity - used, open_pnl }))
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
    let expires_in = st.sso.mint(claims.jti.clone(), user).await;
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
