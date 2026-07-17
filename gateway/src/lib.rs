//! # gateway
//!
//! Сетевой шлюз биржи: REST + WS поверх [`orchestrator::Orchestrator`] (ADR-010), с market data
//! (свечи, инструменты — ADR-012) и демо-симулятором рынка. Доменные типы наружу не протекают —
//! на границе локальные DTO. Оркестратор — единственный писатель, за `Mutex`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

use exchange_core::Event;
use marketdata::{Candle, CandleStore};
use orchestrator::{OrderReject, Orchestrator};

/// Таймфреймы свечей (секунды): 1m,5m,15m,30m,1h,4h,1d.
pub const TIMEFRAMES: [u32; 7] = [60, 300, 900, 1800, 3600, 14400, 86400];

// ============================ Состояние ====================================

/// Реестр пользователей dev-уровня: токен → UserId (ADR-010, не продакшн).
#[derive(Debug, Default)]
pub struct UserRegistry {
    by_token: HashMap<String, UserId>,
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
}

/// Метаданные инструмента для витрины (левый список, масштабы клиента).
#[derive(Clone)]
pub struct InstrumentMeta {
    pub id: u32,
    pub symbol: String,
    pub base: u32,
    pub quote: u32,
    pub price_decimals: u8,
    pub qty_decimals: u8,
}

#[derive(Clone)]
pub struct AppState {
    orch: Arc<Mutex<Orchestrator>>,
    users: Arc<Mutex<UserRegistry>>,
    candles: Arc<Mutex<CandleStore>>,
    instruments: Arc<Mutex<Vec<InstrumentMeta>>>,
    order_seq: Arc<AtomicU64>,
    events_tx: broadcast::Sender<String>,
}

pub fn build_state() -> AppState {
    let (events_tx, _rx) = broadcast::channel(2048);
    AppState {
        orch: Arc::new(Mutex::new(Orchestrator::new())),
        users: Arc::new(Mutex::new(UserRegistry::default())),
        candles: Arc::new(Mutex::new(CandleStore::new(TIMEFRAMES.to_vec(), 1500))),
        instruments: Arc::new(Mutex::new(Vec::new())),
        order_seq: Arc::new(AtomicU64::new(1)),
        events_tx,
    }
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Учесть события: сделки → свечи, затем разослать в живую ленту.
async fn publish(st: &AppState, events: &[Event]) {
    let ts = now_secs();
    {
        let mut cs = st.candles.lock().await;
        for e in events {
            if let Event::Trade { instrument, price, qty, .. } = e {
                cs.ingest(instrument.0, ts, price.0, qty.0);
            }
        }
    }
    let dtos: Vec<EventDto> = events.iter().map(EventDto::from).collect();
    if let Ok(json) = serde_json::to_string(&dtos) {
        let _ = st.events_tx.send(json);
    }
}

/// Зарегистрировать инструмент в движке и в витрине.
pub async fn add_instrument(st: &AppState, m: InstrumentMeta) {
    {
        let mut o = st.orch.lock().await;
        o.register_instrument(Instrument {
            id: InstrumentId(m.id),
            symbol: m.symbol.clone(),
            base: AssetId(m.base),
            quote: AssetId(m.quote),
            price_decimals: m.price_decimals,
            qty_decimals: m.qty_decimals,
            tick_size: Price(1),
            lot_size: Qty(1),
            min_qty: Qty(1),
        });
    }
    st.instruments.lock().await.push(m);
}

// ============================ Роутер =======================================

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/admin/users", post(create_user))
        .route("/admin/deposit", post(deposit))
        .route("/instruments", get(list_instruments))
        .route("/candles/:instrument", get(get_candles))
        .route("/orders", post(place_order))
        .route("/orders/:id", delete(cancel_order))
        .route("/book/:instrument", get(get_book))
        .route("/balance/:asset", get(get_balance))
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
impl From<&Candle> for CandleDto {
    fn from(c: &Candle) -> Self {
        CandleDto { time: c.time, open: c.open, high: c.high, low: c.low, close: c.close, volume: c.volume }
    }
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

async fn authed(st: &AppState, headers: &HeaderMap) -> Result<UserId, ApiErr> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "missing bearer token"))?;
    let users = st.users.lock().await;
    users.resolve(token).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "invalid token"))
}

async fn create_user(State(st): State<AppState>, Json(req): Json<CreateUserReq>) -> Result<Json<CreateUserResp>, ApiErr> {
    let mut users = st.users.lock().await;
    Ok(Json(CreateUserResp { user_id: users.create(req.token).0 }))
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

async fn list_instruments(State(st): State<AppState>) -> Json<Vec<InstrumentDto>> {
    let metas = st.instruments.lock().await.clone();
    let o = st.orch.lock().await;
    let cs = st.candles.lock().await;
    let mut out = Vec::with_capacity(metas.len());
    for m in metas {
        let snap = o.snapshot(InstrumentId(m.id), 1);
        let (bid, ask) = snap
            .map(|s| (s.bids.first().map(|l| l.price.0), s.asks.first().map(|l| l.price.0)))
            .unwrap_or((None, None));
        let series = cs.candles(m.id, 60, 240);
        let last = series.last().map(|c| c.close);
        let first = series.first().map(|c| c.open);
        let high = series.iter().map(|c| c.high).max();
        let change = match (first, last) {
            (Some(f), Some(l)) if f != 0 => (l - f) as f64 / f as f64 * 100.0,
            _ => 0.0,
        };
        out.push(InstrumentDto {
            id: m.id, symbol: m.symbol, base: m.base, quote: m.quote,
            price_decimals: m.price_decimals, qty_decimals: m.qty_decimals,
            last, bid, ask, high, change,
        });
    }
    Json(out)
}

async fn get_candles(State(st): State<AppState>, Path(instrument): Path<u32>, Query(q): Query<CandleQuery>) -> Json<Vec<CandleDto>> {
    let tf = q.tf.unwrap_or(60);
    let limit = q.limit.unwrap_or(300).min(1500);
    let cs = st.candles.lock().await;
    Json(cs.candles(instrument, tf, limit).iter().map(CandleDto::from).collect())
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

// ============================ Демо-сид + симулятор =========================

/// Минимальный сид для тестов: инструмент 1 (BTC-USDT), пользователи alice/bob с депозитами.
pub async fn seed_demo(st: &AppState) {
    let (alice, bob) = {
        let mut users = st.users.lock().await;
        (users.create("alice-token".into()), users.create("bob-token".into()))
    };
    add_instrument(st, InstrumentMeta { id: 1, symbol: "BTC-USDT".into(), base: 1, quote: 2, price_decimals: 2, qty_decimals: 3 }).await;
    let mut o = st.orch.lock().await;
    for u in [alice, bob] {
        o.deposit(u, AssetId(1), Amount(1_000_000));
        o.deposit(u, AssetId(2), Amount(100_000_000));
    }
}

/// Демо-инструмент для симулятора: базовая цена и параметры блуждания (в raw-единицах).
#[derive(Clone)]
struct SimCfg {
    id: u32,
    base_price: i64,
    spread: i64,
    step: i64,
    qty: i64,
}

/// Определение демо-инструмента для сида/симулятора.
struct Def {
    id: u32,
    symbol: &'static str,
    base: u32,
    price_decimals: u8,
    base_price: i64,
    spread: i64,
    step: i64,
    qty: i64,
}

/// Полный демо-рынок: несколько пар + sim-пользователи + бэкфилл истории + запуск симулятора.
pub async fn seed_market(st: &AppState) {
    let defs = [
        Def { id: 1, symbol: "BTC-USDT", base: 1, price_decimals: 2, base_price: 6_200_000, spread: 300, step: 400, qty: 1 },
        Def { id: 2, symbol: "ETH-USDT", base: 3, price_decimals: 2, base_price: 340_000, spread: 40, step: 60, qty: 5 },
        Def { id: 3, symbol: "SOL-USDT", base: 4, price_decimals: 2, base_price: 15_000, spread: 5, step: 8, qty: 20 },
        Def { id: 4, symbol: "XRP-USDT", base: 5, price_decimals: 4, base_price: 6_000, spread: 3, step: 5, qty: 50 },
        Def { id: 5, symbol: "DOGE-USDT", base: 6, price_decimals: 5, base_price: 15_000, spread: 5, step: 9, qty: 100 },
    ];
    // Инструмент 1 уже добавлен seed_demo; добавляем остальные.
    for d in defs.iter() {
        if d.id != 1 {
            add_instrument(st, InstrumentMeta { id: d.id, symbol: d.symbol.into(), base: d.base, quote: 2, price_decimals: d.price_decimals, qty_decimals: 3 }).await;
        }
    }

    // Sim-пользователи с большими балансами по всем активам.
    let (maker, taker) = {
        let mut users = st.users.lock().await;
        (users.create("sim-maker".into()), users.create("sim-taker".into()))
    };
    {
        let mut o = st.orch.lock().await;
        for u in [maker, taker] {
            for asset in [1u32, 2, 3, 4, 5, 6] {
                o.deposit(u, AssetId(asset), Amount(1_000_000_000_000));
            }
        }
    }

    // Синтетический бэкфилл истории свечей (демо, не реальные котировки).
    let now = now_secs();
    let mut rng = Xorshift::new(0xC0FFEE ^ now as u64);
    {
        let mut cs = st.candles.lock().await;
        for d in defs.iter() {
            for &tf in TIMEFRAMES.iter() {
                let series = gen_history(d.base_price, tf, 240, now, &mut rng);
                cs.seed(d.id, tf, series);
            }
        }
    }

    // Запускаем симулятор.
    let cfgs: Vec<SimCfg> = defs.iter().map(|d| SimCfg { id: d.id, base_price: d.base_price, spread: d.spread, step: d.step, qty: d.qty }).collect();
    spawn_simulator(st.clone(), cfgs, maker, taker);
}

/// Синтетическая история свечей (случайное блуждание вокруг base). Демо-данные.
fn gen_history(base: i64, tf: u32, count: usize, now: i64, rng: &mut Xorshift) -> Vec<Candle> {
    let step = (base / 300).max(1);
    let start = CandleStore::bucket(now, tf) - (count as i64 - 1) * tf as i64;
    let mut price = base;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let time = start + i as i64 * tf as i64;
        let open = price;
        let (mut high, mut low, mut close) = (open, open, open);
        for _ in 0..5 {
            let d = (rng.next() % (2 * step as u64 + 1)) as i64 - step;
            close = (close + d).max(1);
            high = high.max(close);
            low = low.min(close);
        }
        price = close;
        out.push(Candle { time, open, high, low, close, volume: (rng.next() % 1000) as i64 });
    }
    out
}

/// Демо-симулятор: по каждой паре двигает котировки и делает встречные сделки (ADR-012).
fn spawn_simulator(st: AppState, cfgs: Vec<SimCfg>, maker: UserId, taker: UserId) {
    struct Sim {
        cfg: SimCfg,
        mid: i64,
        ask_id: Option<OrderId>,
        bid_id: Option<OrderId>,
    }
    tokio::spawn(async move {
        let mut rng = Xorshift::new(0x1234_ABCD ^ now_secs() as u64);
        let mut sims: Vec<Sim> = cfgs.into_iter().map(|c| Sim { mid: c.base_price, cfg: c, ask_id: None, bid_id: None }).collect();
        let mut ticker = tokio::time::interval(Duration::from_millis(700));
        loop {
            ticker.tick().await;
            for s in sims.iter_mut() {
                let step = s.cfg.step;
                let d = (rng.next() % (2 * step as u64 + 1)) as i64 - step;
                let lo = (s.cfg.base_price - s.cfg.base_price / 5).max(1);
                let hi = s.cfg.base_price + s.cfg.base_price / 5;
                s.mid = (s.mid + d).clamp(lo, hi);
                let inst = InstrumentId(s.cfg.id);
                let ask_px = Price(s.mid + s.cfg.spread);
                let bid_px = Price((s.mid - s.cfg.spread).max(1));
                // Мейкер крупнее рыночной заявки, чтобы обе стороны стакана оставались видимыми.
                let q = Qty((3 + (rng.next() % 3) as i64) * s.cfg.qty);
                let mq = Qty((1 + (rng.next() % 2) as i64) * s.cfg.qty);

                let events = {
                    let mut o = st.orch.lock().await;
                    if let Some(id) = s.ask_id.take() {
                        let _ = o.cancel(maker, inst, id);
                    }
                    if let Some(id) = s.bid_id.take() {
                        let _ = o.cancel(taker, inst, id);
                    }
                    let ask_id = OrderId(st.order_seq.fetch_add(1, Ordering::SeqCst));
                    let _ = o.place_limit(maker, inst, ask_id, Side::Sell, ask_px, q, TimeInForce::Gtc);
                    s.ask_id = Some(ask_id);
                    let bid_id = OrderId(st.order_seq.fetch_add(1, Ordering::SeqCst));
                    let _ = o.place_limit(taker, inst, bid_id, Side::Buy, bid_px, q, TimeInForce::Gtc);
                    s.bid_id = Some(bid_id);

                    let mid_id = OrderId(st.order_seq.fetch_add(1, Ordering::SeqCst));
                    let r = if (rng.next() & 1) == 0 {
                        o.place_market(taker, inst, mid_id, Side::Buy, mq)
                    } else {
                        o.place_market(maker, inst, mid_id, Side::Sell, mq)
                    };
                    r.unwrap_or_default()
                };
                publish(&st, &events).await;
            }
        }
    });
}

/// Крошечный детерминированный ГПСЧ (для демо-данных на границе; в ядро не входит).
struct Xorshift(u64);
impl Xorshift {
    fn new(seed: u64) -> Self {
        Xorshift(seed | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}
