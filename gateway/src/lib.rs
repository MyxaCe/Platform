//! # gateway
//!
//! Сетевой шлюз биржи: REST для команд/чтения + WS для живой ленты событий, поверх
//! [`orchestrator::Orchestrator`] (ADR-010). Доменные типы наружу не протекают — на границе
//! используются локальные DTO с `serde`.
//!
//! Оркестратор — единственный писатель, поэтому за `Mutex`; каждый запрос коротко берёт лок.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
use orchestrator::{OrderReject, Orchestrator};

// ============================ Состояние ====================================

/// Реестр пользователей dev-уровня: токен → UserId (см. ADR-010, не продакшн).
#[derive(Debug, Default)]
pub struct UserRegistry {
    by_token: HashMap<String, UserId>,
    next: u64,
}

impl UserRegistry {
    /// Создать пользователя с заданным токеном (dev). Возвращает id.
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

/// Разделяемое состояние приложения (клонируется в каждый хендлер; поля — за Arc).
#[derive(Clone)]
pub struct AppState {
    orch: Arc<Mutex<Orchestrator>>,
    users: Arc<Mutex<UserRegistry>>,
    order_seq: Arc<AtomicU64>,
    events_tx: broadcast::Sender<String>,
}

pub fn build_state() -> AppState {
    let (events_tx, _rx) = broadcast::channel(1024);
    AppState {
        orch: Arc::new(Mutex::new(Orchestrator::new())),
        users: Arc::new(Mutex::new(UserRegistry::default())),
        order_seq: Arc::new(AtomicU64::new(1)),
        events_tx,
    }
}

/// Наполнить демо-данными: инструмент BTC-USDT, пользователи alice/bob с депозитами.
pub async fn seed_demo(st: &AppState) {
    let (alice, bob) = {
        let mut users = st.users.lock().await;
        (users.create("alice-token".into()), users.create("bob-token".into()))
    };
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
        o.deposit(u, AssetId(1), Amount(1_000_000)); // BTC
        o.deposit(u, AssetId(2), Amount(100_000_000)); // USDT
    }
}

// ============================ Роутер =======================================

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/admin/users", post(create_user))
        .route("/admin/instruments", post(register_instrument))
        .route("/admin/deposit", post(deposit))
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
struct InstrumentReq {
    id: u32,
    symbol: String,
    base: u32,
    quote: u32,
    price_decimals: u8,
    qty_decimals: u8,
    tick_size: i64,
    lot_size: i64,
    min_qty: i64,
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

/// JSON-представление события движка (граница; доменные типы не протекают).
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
    let id = users.create(req.token);
    Ok(Json(CreateUserResp { user_id: id.0 }))
}

async fn register_instrument(State(st): State<AppState>, Json(req): Json<InstrumentReq>) -> Result<StatusCode, ApiErr> {
    if req.tick_size <= 0 || req.lot_size <= 0 || req.min_qty <= 0 {
        return Err(err(StatusCode::BAD_REQUEST, "tick/lot/min must be > 0"));
    }
    let mut o = st.orch.lock().await;
    o.register_instrument(Instrument {
        id: InstrumentId(req.id),
        symbol: req.symbol,
        base: AssetId(req.base),
        quote: AssetId(req.quote),
        price_decimals: req.price_decimals,
        qty_decimals: req.qty_decimals,
        tick_size: Price(req.tick_size),
        lot_size: Qty(req.lot_size),
        min_qty: Qty(req.min_qty),
    });
    Ok(StatusCode::CREATED)
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
            let dtos: Vec<EventDto> = events.iter().map(EventDto::from).collect();
            // Публикуем в живую ленту (ошибку отсутствия подписчиков игнорируем).
            if let Ok(json) = serde_json::to_string(&dtos) {
                let _ = st.events_tx.send(json);
            }
            Ok(Json(PlaceResp { order_id: order_id.0, events: dtos }))
        }
        Err(rej) => Err(reject_to_http(rej)),
    }
}

async fn cancel_order(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    Query(q): Query<CancelQuery>,
) -> Result<Json<Vec<EventDto>>, ApiErr> {
    let user = authed(&st, &headers).await?;
    let result = {
        let mut o = st.orch.lock().await;
        o.cancel(user, InstrumentId(q.instrument), OrderId(id))
    };
    match result {
        Ok(events) => {
            let dtos: Vec<EventDto> = events.iter().map(EventDto::from).collect();
            if let Ok(json) = serde_json::to_string(&dtos) {
                let _ = st.events_tx.send(json);
            }
            Ok(Json(dtos))
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
