use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ws::{Message as WsMessage, WebSocketUpgrade};
use axum::extract::{Path as AxPath, Query as AxQuery, State as AxState};
use axum::http::{HeaderMap as AxHeaders, Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use broker::journal::{AccountEvent, Journal, Operator, Origin};
use domain::account::UserId;

use crate::instruments::{self, Applied, CmsEndpoint, Mode, Platform, Status, V2Endpoint};
use crate::{
    build_state, build_state_with, feed, feed_by_id, feed_v2, monitor_tick, restore_state, router,
    sso, AppState, TenantSource, DEFAULT_USER,
};

const FIXTURE_SQL: &str = include_str!("../tests/fixtures/journal-82bf4a0/data.sql");
const FIXTURE_EXPECTED: &str = include_str!("../tests/fixtures/journal-82bf4a0/expected.txt");
const FIXTURE_USERS: [u64; 3] = [910001, 910002, 910003];

fn summary_of(
    b: &broker::Broker,
    js: &HashMap<UserId, Journal>,
    q: &std::collections::HashSet<UserId>,
    user: UserId,
    sym: &dyn Fn(u32) -> String,
) -> String {
    let raw = b.snapshot(user);
    let mut s = raw.clone();
    s.positions.sort_by_key(|p| p.id);
    s.pendings.sort_by_key(|p| p.id);
    let mut out = String::new();
    out += &format!("user={} quarantined={}\n", user.0, q.contains(&user));
    out += &format!(
        "profile={:?} balance={} next_id={} model={:?} leverage={:?} fees={:?}\n",
        s.profile, s.balance, s.next_id, s.model, s.leverage, s.fees
    );
    for p in &s.positions {
        out += &format!("position {:?} symbol={}\n", p, sym(p.instrument));
    }
    for p in &s.pendings {
        out += &format!("pending {:?} symbol={}\n", p, sym(p.instrument));
    }
    for c in &s.closed {
        out += &format!("closed {:?} symbol={}\n", c, sym(c.instrument));
    }
    let empty = Journal::new(b.start_balance());
    let j = js.get(&user).unwrap_or(&empty);
    out += &format!(
        "journal entries={} audit={} replayed_balance={}\n",
        j.events().len(),
        broker::journal::audit(j, &raw).tag(),
        j.replay().balance
    );
    for e in j.events() {
        out += &format!("event {} {}\n", e.seq(), e.kind().encode());
    }
    out
}

async fn fixture_summary(st: &AppState) -> String {
    let b = st.broker.lock().await;
    let js = st.journals.lock().await;
    let q = st.quarantined.lock().await;
    let sym = |id: u32| feed_by_id(st, id).map(|f| f.symbol).unwrap_or_default();
    FIXTURE_USERS
        .iter()
        .map(|u| summary_of(&b, &js, &q, UserId(*u), &sym))
        .collect()
}

fn sql_values(line: &str) -> Vec<Option<String>> {
    let start = line.find("VALUES (").expect("строка INSERT") + "VALUES (".len();
    let body = &line[start..line.rfind(");").expect("конец INSERT")];
    let mut out = Vec::new();
    let mut chars = body.chars().peekable();
    loop {
        while chars.peek() == Some(&' ') {
            chars.next();
        }
        let Some(&c) = chars.peek() else { break };
        if c == '\'' {
            chars.next();
            let mut v = String::new();
            loop {
                match chars.next() {
                    Some('\'') if chars.peek() == Some(&'\'') => {
                        chars.next();
                        v.push('\'');
                    }
                    Some('\'') => break,
                    Some(ch) => v.push(ch),
                    None => panic!("незакрытая строка"),
                }
            }
            out.push(Some(v));
        } else {
            let mut v = String::new();
            while let Some(&ch) = chars.peek() {
                if ch == ',' {
                    break;
                }
                v.push(ch);
                chars.next();
            }
            let v = v.trim().to_string();
            out.push((v != "NULL").then_some(v));
        }
        while chars.peek() == Some(&' ') {
            chars.next();
        }
        if chars.next().is_none() {
            break;
        }
    }
    out
}

#[test]
fn the_old_journal_folds_into_the_same_balances_without_a_database() {
    let mut by_user: HashMap<u64, Vec<AccountEvent>> = HashMap::new();
    for line in FIXTURE_SQL
        .lines()
        .filter(|l| l.starts_with("INSERT INTO account_events"))
    {
        let v = sql_values(line);
        let get = |i: usize| v[i].clone().unwrap_or_default();
        let origin = match get(3).as_str() {
            "system" => Origin::System,
            "client" => Origin::Client,
            "operator" => Origin::Operator(Operator::new(&get(4), &get(5), &get(6)).unwrap()),
            other => panic!("источник {other}"),
        };
        let e = AccountEvent::from_storage(
            get(1).parse().unwrap(),
            get(2).parse().unwrap(),
            origin,
            &get(7),
            u128::from_str_radix(&get(8), 16).unwrap(),
            u128::from_str_radix(&get(9), 16).unwrap(),
        )
        .unwrap();
        by_user.entry(get(0).parse().unwrap()).or_default().push(e);
    }
    assert_eq!(by_user.len(), 3);
    for user in FIXTURE_USERS {
        let events = by_user.remove(&user).unwrap();
        let j = Journal::load(10_000_000, events).expect("журнал 82bf4a0 читается без карантина");
        let section: Vec<&str> = FIXTURE_EXPECTED
            .split("user=")
            .find(|s| s.starts_with(&format!("{user} ")))
            .unwrap()
            .lines()
            .collect();
        let replayed = section
            .iter()
            .find_map(|l| l.split("replayed_balance=").nth(1))
            .unwrap();
        assert_eq!(j.replay().balance.to_string(), replayed);
        let expected_events: Vec<&str> = section
            .iter()
            .filter_map(|l| l.strip_prefix("event "))
            .collect();
        let got: Vec<String> = j
            .events()
            .iter()
            .map(|e| format!("{} {}", e.seq(), e.kind().encode()))
            .collect();
        assert_eq!(got, expected_events);
    }
}

#[test]
fn the_thirty_pairs_keep_their_ids_symbols_and_precision() {
    let rows = feed::legacy_rows();
    assert_eq!(rows.len(), 30);
    let expected: [(&str, &str, u8, u8); 30] = [
        ("BTCUSD", "BTCUSDT", 2, 5),
        ("ETHUSD", "ETHUSDT", 2, 4),
        ("BNBUSD", "BNBUSDT", 2, 3),
        ("SOLUSD", "SOLUSDT", 2, 3),
        ("XRPUSD", "XRPUSDT", 4, 1),
        ("DOGEUSD", "DOGEUSDT", 5, 0),
        ("ADAUSD", "ADAUSDT", 4, 1),
        ("TRXUSD", "TRXUSDT", 5, 0),
        ("AVAXUSD", "AVAXUSDT", 2, 2),
        ("LINKUSD", "LINKUSDT", 3, 2),
        ("DOTUSD", "DOTUSDT", 3, 2),
        ("LTCUSD", "LTCUSDT", 2, 3),
        ("BCHUSD", "BCHUSDT", 2, 3),
        ("UNIUSD", "UNIUSDT", 3, 2),
        ("ATOMUSD", "ATOMUSDT", 3, 2),
        ("XLMUSD", "XLMUSDT", 5, 0),
        ("ETCUSD", "ETCUSDT", 3, 2),
        ("NEARUSD", "NEARUSDT", 3, 1),
        ("FILUSD", "FILUSDT", 3, 2),
        ("APTUSD", "APTUSDT", 3, 2),
        ("ARBUSD", "ARBUSDT", 4, 1),
        ("OPUSD", "OPUSDT", 4, 1),
        ("INJUSD", "INJUSDT", 3, 2),
        ("AAVEUSD", "AAVEUSDT", 2, 3),
        ("HBARUSD", "HBARUSDT", 5, 0),
        ("VETUSD", "VETUSDT", 5, 0),
        ("ICPUSD", "ICPUSDT", 3, 2),
        ("SHIBUSD", "SHIBUSDT", 8, 0),
        ("SANDUSD", "SANDUSDT", 4, 0),
        ("MANAUSD", "MANAUSDT", 4, 0),
    ];
    for (i, (r, e)) in rows.iter().zip(expected.iter()).enumerate() {
        assert_eq!(r.id as usize, i + 1);
        assert_eq!(
            (
                r.symbol.as_str(),
                r.provider_symbol.as_str(),
                r.price_decimals,
                r.qty_decimals
            ),
            *e
        );
        assert_eq!(r.source, "v1");
    }
}

#[tokio::test]
async fn startup_pins_the_thirty_pairs_and_refuses_a_conflicting_registry() {
    let st = build_state();
    restore_state(&st).await.unwrap();
    restore_state(&st).await.unwrap();
    let rows = st.store.load_instruments().await.unwrap();
    assert_eq!(rows, feed::legacy_rows());

    let store = Arc::new(persistence::MemoryStore::new());
    persistence::Persistence::pin_instruments(
        store.as_ref(),
        &[persistence::InstrumentRow {
            id: 1,
            symbol: "BTCUSD".into(),
            source: "v1".into(),
            provider_symbol: "BTCUSDT".into(),
            price_decimals: 3,
            qty_decimals: 5,
        }],
    )
    .await
    .unwrap();
    let st = build_state_with(store);
    let e = restore_state(&st).await.expect_err("чужая точность у id 1");
    assert!(e.0.contains("BTCUSD"), "{e}");
}

#[tokio::test]
#[ignore = "нужна живая БД: TEST_DATABASE_URL=postgres://... cargo test -p gateway -- --ignored"]
async fn postgres_journal_written_by_82bf4a0_restores_with_the_same_numbers() {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL не задан — тест запущен без базы, которую он проверяет");
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    sqlx::raw_sql("DROP SCHEMA IF EXISTS wave2fx CASCADE; CREATE SCHEMA wave2fx;")
        .execute(&admin)
        .await
        .unwrap();
    let sep = if url.contains('?') { '&' } else { '?' };
    let scoped = format!("{url}{sep}options=-c%20search_path%3Dwave2fx");
    let store = persistence::PgStore::connect(&scoped).await.unwrap();
    persistence::Persistence::init(&store).await.unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&scoped)
        .await
        .unwrap();
    sqlx::raw_sql(FIXTURE_SQL).execute(&pool).await.unwrap();

    let store: Arc<dyn persistence::Persistence> = Arc::new(store);
    for round in 0..2 {
        let st = build_state_with(store.clone());
        restore_state(&st).await.unwrap();
        assert_eq!(
            fixture_summary(&st).await,
            FIXTURE_EXPECTED,
            "подъём {round}: числа журнала 82bf4a0 обязаны совпасть"
        );
        let b = st.broker.lock().await;
        for u in FIXTURE_USERS {
            let s = b.snapshot(UserId(u));
            for (inst, pd, qd) in s
                .positions
                .iter()
                .map(|p| (p.instrument, p.pd, p.qd))
                .chain(s.pendings.iter().map(|p| (p.instrument, p.pd, p.qd)))
            {
                let f = feed_by_id(&st, inst).unwrap();
                assert_eq!(
                    (f.price_decimals, f.qty_decimals),
                    (pd, qd),
                    "цены фида {} обязаны быть в точности записанных позиций",
                    f.canonical
                );
            }
        }
    }
    let ids: Vec<(i32, String)> = sqlx::query_as("SELECT id, symbol FROM instruments ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(ids.len(), 30);
    assert_eq!(ids[0], (1, "BTCUSD".to_string()));
    assert_eq!(ids[27], (28, "SHIBUSD".to_string()));
    sqlx::raw_sql("DROP SCHEMA wave2fx CASCADE")
        .execute(&admin)
        .await
        .unwrap();
}

#[derive(Clone)]
struct Fake {
    v2_instruments: Arc<std::sync::Mutex<Value>>,
    platform: Arc<std::sync::Mutex<Value>>,
    sites: Arc<std::sync::Mutex<HashMap<String, Value>>>,
    ops: Arc<std::sync::Mutex<Vec<Value>>>,
    push: tokio::sync::broadcast::Sender<String>,
}

fn exchange_info() -> Value {
    let pair = |s: &str, b: &str, tick: &str, step: &str| {
        json!({"symbol": s, "baseAsset": b, "quoteAsset": "USDT", "status": "TRADING",
               "filters": [{"filterType": "PRICE_FILTER", "tickSize": tick},
                           {"filterType": "LOT_SIZE", "stepSize": step, "minQty": step}]})
    };
    json!({"symbols": [
        pair("BTCUSDT", "BTC", "0.01000000", "0.00001000"),
        pair("ETHUSDT", "ETH", "0.01000000", "0.00010000"),
        pair("AVAXUSDT", "AVAX", "0.00100000", "0.01000000"),
        pair("LTCUSDT", "LTC", "0.01000000", "0.00100000"),
        pair("EURUSDT", "EUR", "0.00010000", "0.10000000"),
        {"symbol": "NOLOTUSDT", "baseAsset": "NOLOT", "quoteAsset": "USDT", "status": "TRADING",
         "filters": [{"filterType": "PRICE_FILTER", "tickSize": "0.01"}]}
    ]})
}

fn v2_catalog(eur_tick: &str) -> Value {
    json!({"instruments": [
        {"symbol": "EURUSD", "name": "Euro / US Dollar", "category": "forex", "base": "EUR", "quote": "USD",
         "priceDecimals": 5, "tickSize": eur_tick, "qtyDecimals": 2, "qtyStep": "0.01", "minQty": "0.01",
         "contractSize": "100000", "hasDepth": false, "hasTrades": false, "has1s": false,
         "session": {"timezone": "America/New_York", "weekly": [{"open": "sun 17:00", "close": "fri 17:00"}], "holidays": ["2026-12-25"]},
         "status": "closed"},
        {"symbol": "XAUUSD", "name": "Gold", "category": "metals", "priceDecimals": 2, "tickSize": "0.01",
         "qtyDecimals": 2, "qtyStep": "0.01", "minQty": "0.01", "contractSize": "100",
         "hasDepth": false, "hasTrades": false, "has1s": false, "status": "open"},
        {"symbol": "AAPL", "name": "Apple", "category": "stocks", "priceDecimals": 2, "tickSize": "0.01",
         "qtyDecimals": 0, "qtyStep": "1", "minQty": "1", "contractSize": "1",
         "hasDepth": false, "hasTrades": true, "has1s": false, "status": "open"},
        {"symbol": "BTCX", "category": "crypto", "tickSize": "0.01", "qtyStep": "1"}
    ]})
}

fn list(version: i64, items: &[(&str, &str)]) -> Value {
    json!({"version": version, "updatedAt": "2026-11-21T10:00:00Z",
           "instruments": items.iter().map(|(s, m)| json!({"symbol": s, "mode": m})).collect::<Vec<_>>()})
}

async fn ws_handler(AxState(f): AxState<Fake>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |mut socket| async move {
        let mut rx = f.push.subscribe();
        loop {
            tokio::select! {
                msg = socket.recv() => match msg {
                    Some(Ok(WsMessage::Text(t))) => {
                        let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                        let id = v["id"].clone();
                        f.ops.lock().unwrap().push(v);
                        let _ = socket.send(WsMessage::Text(json!({"type": "ack", "id": id}).to_string())).await;
                    }
                    Some(Ok(_)) => {}
                    _ => break,
                },
                out = rx.recv() => match out {
                    Ok(t) => { let _ = socket.send(WsMessage::Text(t)).await; }
                    Err(_) => break,
                },
            }
        }
    })
}

async fn candles(AxQuery(q): AxQuery<HashMap<String, String>>) -> Json<Value> {
    let n: usize = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(2);
    let rows: Vec<Value> = (0..n.min(2))
        .map(|i| {
            json!([
                1_795_078_800_000i64 + i as i64 * 3_600_000,
                "1.08401",
                "1.08455",
                "1.08390",
                "1.08412",
                null
            ])
        })
        .collect();
    Json(Value::Array(rows))
}

async fn site(
    AxState(f): AxState<Fake>,
    headers: AxHeaders,
    AxPath(slug): AxPath<String>,
) -> axum::response::Response {
    if headers.get("x-api-key").and_then(|v| v.to_str().ok()) != Some("k") {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match f.sites.lock().unwrap().get(&slug) {
        Some(v) => Json(v.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn platform_list(AxState(f): AxState<Fake>, headers: AxHeaders) -> axum::response::Response {
    if headers.get("x-api-key").and_then(|v| v.to_str().ok()) != Some("k") {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(f.platform.lock().unwrap().clone()).into_response()
}

async fn fake() -> (Fake, String) {
    let f = Fake {
        v2_instruments: Arc::new(std::sync::Mutex::new(v2_catalog("0.00001"))),
        platform: Arc::new(std::sync::Mutex::new(list(1, &[("BTCUSD", "trade")]))),
        sites: Arc::new(std::sync::Mutex::new(HashMap::new())),
        ops: Arc::new(std::sync::Mutex::new(Vec::new())),
        push: tokio::sync::broadcast::channel(64).0,
    };
    let app = Router::new()
        .route("/api/v3/exchangeInfo", get(|| async { Json(exchange_info()) }))
        .route(
            "/v2/instruments",
            get(|AxState(f): AxState<Fake>| async move { Json(f.v2_instruments.lock().unwrap().clone()) }),
        )
        .route("/v2/candles", get(candles))
        .route(
            "/v2/quotes",
            get(|| async { Json(json!({"quotes": [{"s": "EURUSD", "b": "1.08400", "a": "1.08410", "t": 1, "status": "closed"}]})) }),
        )
        .route("/v2/stream", get(ws_handler))
        .route("/cms/platform/instruments", get(platform_list))
        .route("/cms/sites/:slug", get(site))
        .with_state(f.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    (f, format!("http://{addr}"))
}

fn cache_path(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "terminal-wave2-{}-{name}-{}",
        std::process::id(),
        crate::now_ms()
    ));
    p.join("platform-instruments.json")
}

fn platform_for(
    base: &str,
    cms: bool,
    cache: Option<std::path::PathBuf>,
    reject: bool,
) -> Platform {
    let mut p = Platform::new(
        cms.then(|| CmsEndpoint {
            base: base.to_string(),
            api_key: "k".into(),
        }),
        base.to_string(),
        Some(V2Endpoint {
            rest: base.to_string(),
            ws: format!("{}/v2/stream", base.replace("http://", "ws://")),
        }),
        cache,
    );
    p.webhook_secret = Some("whsec".into());
    p.reject_close_only = reject;
    p
}

async fn state_on(store: Arc<dyn persistence::Persistence>, p: Platform) -> AppState {
    let mut st = build_state_with(store);
    st.platform = Arc::new(p);
    restore_state(&st).await.unwrap();
    st
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn id_of(st: &AppState, symbol: &str) -> u32 {
    st.catalog.read().unwrap().id_of(symbol).unwrap()
}

async fn quote(st: &AppState, symbol: &str, b: &str, a: &str) {
    let index = feed_v2::index_of(st);
    feed_v2::handle_message(
        st,
        &index,
        &json!({"type": "quote", "s": symbol, "b": b, "a": a, "t": 1}).to_string(),
    )
    .await;
}

async fn status(st: &AppState, symbol: &str, s: &str, next_open: Option<i64>) {
    let index = feed_v2::index_of(st);
    feed_v2::handle_message(
        st,
        &index,
        &json!({"type": "status", "s": symbol, "status": s, "nextOpen": next_open, "reason": "weekend", "t": 1}).to_string(),
    )
    .await;
}

async fn crypto(st: &AppState, id: u32, bid: i64, ask: i64) {
    st.tickers.lock().await.insert(
        id,
        feed::Ticker {
            last: (bid + ask) / 2,
            change: 0.0,
            high: ask,
            bid,
            ask,
            ..Default::default()
        },
    );
}

#[tokio::test]
async fn the_cms_list_registers_new_instruments_with_permanent_ids_and_fixed_precision() {
    let (f, base) = fake().await;
    *f.platform.lock().unwrap() = list(
        3,
        &[
            ("BTCUSD", "trade"),
            ("EURUSD", "trade"),
            ("XAUUSD", "trade"),
            ("AAPL", "closeOnly"),
            ("LTCUSD", "trade"),
            ("NOLOTUSD", "trade"),
            ("NOPE", "trade"),
        ],
    );
    let cache = cache_path("register");
    let store: Arc<dyn persistence::Persistence> = Arc::new(persistence::MemoryStore::new());
    let st = state_on(
        store.clone(),
        platform_for(&base, true, Some(cache.clone()), false),
    )
    .await;
    assert_eq!(st.catalog.read().unwrap().source, "builtin");
    let applied = instruments::poll_once(&st).await.unwrap();
    assert_eq!(
        applied,
        Applied::Applied {
            active: 5,
            skipped: vec!["NOLOTUSD".into(), "NOPE".into()]
        }
    );
    assert_eq!(
        instruments::poll_once(&st).await.unwrap(),
        Applied::Unchanged
    );

    assert_eq!(id_of(&st, "BTCUSD"), 1);
    assert_eq!(id_of(&st, "LTCUSD"), 12);
    let eur = feed_by_id(&st, id_of(&st, "EURUSD")).unwrap();
    assert!(eur.id > 30);
    assert_eq!(
        eur.source,
        feed::Source::V2,
        "форекс EURUSD — не криптопара EUR/USDT"
    );
    assert_eq!((eur.price_decimals, eur.qty_decimals), (5, 0));
    assert_eq!(
        (eur.qty_step, eur.min_qty, eur.tick_size),
        (Some(1000), Some(1000), Some(1))
    );
    assert_eq!(eur.contract_size.as_deref(), Some("100000"));
    assert_eq!(eur.lot_decimals, Some(2));
    assert!(!eur.has_depth && !eur.has_1s);
    let xau = feed_by_id(&st, id_of(&st, "XAUUSD")).unwrap();
    assert_eq!(
        (xau.price_decimals, xau.qty_decimals, xau.qty_step),
        (2, 0, Some(1))
    );
    let aapl = feed_by_id(&st, id_of(&st, "AAPL")).unwrap();
    assert!(aapl.id > 30 && aapl.id != eur.id && aapl.id != xau.id);
    assert_eq!(st.platform_mode(aapl.id), Mode::CloseOnly);
    assert_eq!(
        st.platform_mode(2),
        Mode::CloseOnly,
        "ETH вне списка — только закрытие"
    );

    let app = router(st.clone());
    let (s, body) = send(&app, "GET", "/instruments", None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    let shown: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["canonical"].as_str().unwrap())
        .collect();
    assert_eq!(shown, vec!["BTCUSD", "EURUSD", "XAUUSD", "AAPL", "LTCUSD"]);
    let e = &body[1];
    assert_eq!(e["symbol"], "EURUSD");
    assert_eq!(e["category"], "forex");
    assert_eq!(e["market"]["status"], "closed");
    assert_eq!(e["market"]["tradable"], false);
    assert_eq!(e["session"]["timezone"], "America/New_York");
    assert_eq!(body[3]["mode"], "closeOnly");
    assert_eq!(body[0]["symbol"], "BTC-USDT");
    assert_eq!(body[0]["market"]["status"], "open");

    let raw = std::fs::read_to_string(&cache).expect("список сохранён на том");
    assert!(raw.contains("\"EURUSD\""));

    *f.v2_instruments.lock().unwrap() = v2_catalog("0.001");
    let again = state_on(
        store.clone(),
        platform_for("http://127.0.0.1:9", true, Some(cache.clone()), false),
    )
    .await;
    assert_eq!(again.catalog.read().unwrap().source, "file");
    assert_eq!(id_of(&again, "EURUSD"), eur.id);
    assert_eq!(
        feed_by_id(&again, eur.id).unwrap().contract_size.as_deref(),
        Some("100000")
    );
    let shifted = state_on(
        store.clone(),
        platform_for(&base, true, Some(cache.clone()), false),
    )
    .await;
    instruments::refresh_entries(&shifted, true).await;
    let kept = feed_by_id(&shifted, eur.id).unwrap();
    assert_eq!(
        (kept.price_decimals, kept.qty_decimals),
        (5, 0),
        "точность хранения не меняется вслед за каталогом"
    );
    let _ = std::fs::remove_dir_all(cache.parent().unwrap());
}

#[tokio::test]
async fn without_cms_and_cache_the_terminal_keeps_the_thirty_pairs() {
    let st = state_on(
        Arc::new(persistence::MemoryStore::new()),
        Platform::new(
            None,
            "http://127.0.0.1:9".into(),
            None,
            Some(cache_path("none")),
        ),
    )
    .await;
    let app = router(st.clone());
    let (_, body) = send(&app, "GET", "/instruments", None, &[]).await;
    let symbols: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["symbol"].as_str().unwrap())
        .collect();
    let before: Vec<String> = feed::instruments().into_iter().map(|f| f.symbol).collect();
    assert_eq!(symbols, before);
    let ids: Vec<u64> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, (1..=30).collect::<Vec<u64>>());
    assert!(body
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["mode"] == "trade" && i["has_depth"] == true));
}

#[tokio::test]
async fn a_closed_market_refuses_market_deals_takes_orders_and_fires_stops_at_the_first_price_after_the_open(
) {
    let (f, base) = fake().await;
    *f.platform.lock().unwrap() = list(1, &[("BTCUSD", "trade"), ("EURUSD", "trade")]);
    let st = state_on(
        Arc::new(persistence::MemoryStore::new()),
        platform_for(&base, true, Some(cache_path("closed")), false),
    )
    .await;
    instruments::poll_once(&st).await.unwrap();
    let eur = id_of(&st, "EURUSD");
    let app = router(st.clone());
    let deal =
        |qty: i64, sl: Option<i64>| json!({"instrument": eur, "side": "buy", "qty": qty, "sl": sl});

    status(&st, "EURUSD", "closed", Some(1_795_284_000_000)).await;
    st.tickers.lock().await.insert(
        eur,
        feed_v2::ticker_from_quote(108_400, 108_410, None, None),
    );
    let (s, b) = send(&app, "POST", "/deals", Some(deal(1000, None)), &[]).await;
    assert_eq!(
        (s, b["error"].as_str()),
        (StatusCode::CONFLICT, Some("market_closed"))
    );
    assert_eq!(b["next_open"], 1_795_284_000_000i64);
    status(&st, "EURUSD", "halted", None).await;
    let (s, b) = send(&app, "POST", "/deals", Some(deal(1000, None)), &[]).await;
    assert_eq!(
        (s, b["error"].as_str()),
        (StatusCode::CONFLICT, Some("market_halted"))
    );

    let (s, _) = send(
        &app,
        "POST",
        "/pending",
        Some(json!({"instrument": eur, "side": "buy", "qty": 1000, "price": 107_000})),
        &[],
    )
    .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "отложенный ордер при закрытом рынке ждёт открытия"
    );

    quote(&st, "EURUSD", "1.08400", "1.08410").await;
    let (s, b) = send(&app, "POST", "/deals", Some(deal(500, None)), &[]).await;
    assert_eq!(
        (s, b["error"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("qty_below_min"))
    );
    let (s, b) = send(&app, "POST", "/deals", Some(deal(1500, None)), &[]).await;
    assert_eq!(
        (s, b["error"].as_str()),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Some("qty_not_multiple_of_step")
        )
    );
    let (s, b) = send(&app, "POST", "/deals", Some(deal(1000, Some(107_500))), &[]).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let deal_id = b["id"].as_u64().unwrap();

    status(&st, "EURUSD", "closed", Some(1_795_284_000_000)).await;
    let (s, b) = send(&app, "POST", &format!("/deals/{deal_id}/close"), None, &[]).await;
    assert_eq!(
        (s, b["error"].as_str()),
        (StatusCode::CONFLICT, Some("market_closed"))
    );

    st.tickers.lock().await.insert(
        eur,
        feed_v2::ticker_from_quote(106_000, 106_010, None, None),
    );
    monitor_tick(&st).await;
    let (_, open) = send(&app, "GET", "/deals", None, &[]).await;
    assert_eq!(
        open.as_array().unwrap().len(),
        1,
        "стоп не срабатывает по цене закрытого рынка"
    );
    let (_, pend) = send(&app, "GET", "/pending", None, &[]).await;
    assert_eq!(pend.as_array().unwrap().len(), 1);

    quote(&st, "EURUSD", "1.06000", "1.06010").await;
    monitor_tick(&st).await;
    let (_, closed) = send(&app, "GET", "/deals/closed", None, &[]).await;
    let c = &closed[0];
    assert_eq!(c["level"], 107_500);
    assert_eq!(
        c["exit"], 106_005,
        "исполнение по первой цене после открытия, с гэпом"
    );
    assert_eq!(c["across_gap"], true);
    let (_, open) = send(&app, "GET", "/deals", None, &[]).await;
    assert_eq!(
        open.as_array().unwrap().len(),
        1,
        "отложенный ордер исполнился после открытия"
    );
    assert!(!st.quarantined.lock().await.contains(&DEFAULT_USER));

    st.markets
        .lock()
        .unwrap()
        .get_mut(&eur)
        .unwrap()
        .last_quote_ms = 1;
    let (s, _) = send(&app, "POST", "/deals", Some(deal(1000, None)), &[]).await;
    assert_eq!(
        s,
        StatusCode::SERVICE_UNAVAILABLE,
        "открытый рынок без свежей цены — не торгуем"
    );

    let (s, b) = send(&app, "GET", &format!("/depth/{eur}"), None, &[]).await;
    assert_eq!(
        (s, b["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("depth_unavailable"))
    );
    let (s, b) = send(&app, "GET", &format!("/candles/{eur}?tf=1"), None, &[]).await;
    assert_eq!(
        (s, b["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("interval_unsupported"))
    );
    let (s, b) = send(
        &app,
        "GET",
        &format!("/candles/{eur}?tf=3600&limit=2&end=1795086000000"),
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b[0]["close"], 108_412);
    assert!(b[0]["volume"].is_null(), "объёма нет — null, а не ноль");
}

#[tokio::test]
async fn close_only_refuses_opening_lets_close_and_holds_or_rejects_reached_orders() {
    for reject in [false, true] {
        let (f, base) = fake().await;
        *f.platform.lock().unwrap() = list(1, &[("BTCUSD", "trade"), ("ETHUSD", "trade")]);
        let st = state_on(
            Arc::new(persistence::MemoryStore::new()),
            platform_for(&base, true, Some(cache_path("closeonly")), reject),
        )
        .await;
        instruments::poll_once(&st).await.unwrap();
        let app = router(st.clone());
        crypto(&st, 2, 320_000, 320_010).await;
        let (s, b) = send(
            &app,
            "POST",
            "/deals",
            Some(json!({"instrument": 2, "side": "buy", "qty": 1000})),
            &[],
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let deal_id = b["id"].as_u64().unwrap();
        let (s, _) = send(
            &app,
            "POST",
            "/pending",
            Some(json!({"instrument": 2, "side": "buy", "qty": 1000, "price": 310_000})),
            &[],
        )
        .await;
        assert_eq!(s, StatusCode::OK);

        *f.platform.lock().unwrap() = list(2, &[("BTCUSD", "trade"), ("ETHUSD", "closeOnly")]);
        instruments::poll_once(&st).await.unwrap();
        let (s, b) = send(
            &app,
            "POST",
            "/deals",
            Some(json!({"instrument": 2, "side": "buy", "qty": 1000})),
            &[],
        )
        .await;
        assert_eq!(
            (s, b["error"].as_str()),
            (StatusCode::CONFLICT, Some("instrument_close_only"))
        );
        let (s, b) = send(
            &app,
            "POST",
            "/pending",
            Some(json!({"instrument": 2, "side": "buy", "qty": 1000, "price": 300_000})),
            &[],
        )
        .await;
        assert_eq!(
            (s, b["error"].as_str()),
            (StatusCode::CONFLICT, Some("instrument_close_only"))
        );

        *f.platform.lock().unwrap() = list(3, &[("BTCUSD", "trade")]);
        instruments::poll_once(&st).await.unwrap();
        assert!(
            st.platform.subscriptions_v1().borrow().contains("ETHUSDT"),
            "инструмент с позицией не отписывается"
        );
        crypto(&st, 2, 305_000, 305_010).await;
        monitor_tick(&st).await;
        let (_, pend) = send(&app, "GET", "/pending", None, &[]).await;
        let (_, open) = send(&app, "GET", "/deals", None, &[]).await;
        assert_eq!(
            open.as_array().unwrap().len(),
            1,
            "новая позиция по closeOnly не открывается"
        );
        if reject {
            assert!(pend.as_array().unwrap().is_empty());
            let (_, j) = send(&app, "GET", "/account/journal", None, &[]).await;
            let last = j["entries"].as_array().unwrap().last().unwrap()["event"]
                .as_str()
                .unwrap()
                .to_string();
            assert!(last.contains("reason=instrument_close_only"), "{last}");
            assert_eq!(j["status"], "reconciled");
        } else {
            assert_eq!(pend.as_array().unwrap().len(), 1, "без флага ордер ждёт");
        }
        let (s, _) = send(&app, "POST", &format!("/deals/{deal_id}/close"), None, &[]).await;
        assert_eq!(s, StatusCode::OK, "закрытие по closeOnly разрешено");
        if !reject {
            let id = pend[0]["id"].as_u64().unwrap();
            let (s, _) = send(&app, "DELETE", &format!("/pending/{id}"), None, &[]).await;
            assert_eq!(s, StatusCode::NO_CONTENT);
        }
        instruments::recompute_subscriptions(&st).await;
        assert!(!st.platform.subscriptions_v1().borrow().contains("ETHUSDT"));

        let again = build_state_with(st.store.clone());
        restore_state(&again).await.unwrap();
        assert!(!again.quarantined.lock().await.contains(&DEFAULT_USER));
    }
}

#[tokio::test]
async fn close_only_is_decided_by_the_clients_own_site() {
    let (f, base) = fake().await;
    *f.platform.lock().unwrap() = list(1, &[("BTCUSD", "trade"), ("ETHUSD", "trade")]);
    f.sites.lock().unwrap().insert(
        "site-a".into(),
        json!({"instruments": ["BTCUSD", "ETHUSD"], "closeOnlyInstruments": ["ETHUSD"]}),
    );
    f.sites.lock().unwrap().insert(
        "site-b".into(),
        json!({"instruments": ["BTCUSD", "ETHUSD"]}),
    );
    let mut st = build_state();
    st.platform = Arc::new(platform_for(&base, true, Some(cache_path("site")), false));
    st.sso = Arc::new(sso::Sso::new("http://jwks.invalid/jwks".into()));
    st.sso_enabled = true;
    st.tenant_instruments = Arc::new(TenantSource::Cms {
        base: base.clone(),
        api_key: "k".into(),
        http: reqwest::Client::new(),
    });
    restore_state(&st).await.unwrap();
    instruments::poll_once(&st).await.unwrap();
    st.sso
        .mint("tok-a".into(), UserId(5001), "site-a".into())
        .await;
    st.sso
        .mint("tok-b".into(), UserId(5002), "site-b".into())
        .await;
    let app = router(st.clone());
    let eth = json!({"instrument": 2, "side": "buy", "qty": 1000});
    let (s, b) = send(
        &app,
        "POST",
        "/deals",
        Some(eth.clone()),
        &[("authorization", "Bearer tok-a")],
    )
    .await;
    assert_eq!(
        (s, b["error"].as_str()),
        (StatusCode::CONFLICT, Some("instrument_close_only"))
    );
    let (s, b) = send(
        &app,
        "POST",
        "/deals",
        Some(eth.clone()),
        &[("authorization", "Bearer tok-b")],
    )
    .await;
    assert_ne!(
        b["error"].as_str(),
        Some("instrument_close_only"),
        "{s} {b}"
    );
    let (_, list_a) = send(&app, "GET", "/instruments?site=site-a", None, &[]).await;
    let modes: Vec<(&str, &str)> = list_a
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["canonical"].as_str().unwrap(),
                i["mode"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(modes, vec![("BTCUSD", "trade"), ("ETHUSD", "closeOnly")]);
}

#[tokio::test]
async fn exposure_answers_the_cms_before_an_instrument_is_removed() {
    let (f, base) = fake().await;
    *f.platform.lock().unwrap() = list(1, &[("BTCUSD", "trade"), ("ETHUSD", "trade")]);
    let mut st = build_state();
    st.platform = Arc::new(platform_for(
        &base,
        true,
        Some(cache_path("exposure")),
        false,
    ));
    st.internal_money_token = Some(Arc::from("t".repeat(40).as_str()));
    restore_state(&st).await.unwrap();
    instruments::poll_once(&st).await.unwrap();
    let app = router(st.clone());
    crypto(&st, 2, 320_000, 320_010).await;
    send(
        &app,
        "POST",
        "/deals",
        Some(json!({"instrument": 2, "side": "buy", "qty": 1000})),
        &[],
    )
    .await;
    send(
        &app,
        "POST",
        "/deals",
        Some(json!({"instrument": 2, "side": "sell", "qty": 1000})),
        &[],
    )
    .await;
    send(
        &app,
        "POST",
        "/pending",
        Some(json!({"instrument": 2, "side": "buy", "qty": 1000, "price": 300_000})),
        &[],
    )
    .await;
    let token = "t".repeat(40);
    let (s, _) = send(
        &app,
        "GET",
        "/internal/instruments/ETHUSD/exposure",
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, b) = send(
        &app,
        "GET",
        "/internal/instruments/ETHUSD/exposure",
        None,
        &[("x-internal-token", &token)],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        b,
        json!({"symbol": "ETHUSD", "openPositions": 2, "pendingOrders": 1})
    );
    let (_, b) = send(
        &app,
        "GET",
        "/internal/instruments/eth-usdt/exposure",
        None,
        &[("x-internal-token", &token)],
    )
    .await;
    assert_eq!(b["openPositions"], 2);
    let (_, b) = send(
        &app,
        "GET",
        "/internal/instruments/NOPE/exposure",
        None,
        &[("x-internal-token", &token)],
    )
    .await;
    assert_eq!(
        b,
        json!({"symbol": "NOPE", "openPositions": 0, "pendingOrders": 0})
    );
}

#[tokio::test]
async fn the_webhook_is_signed_and_only_speeds_up_the_poll() {
    let (f, base) = fake().await;
    let st = state_on(
        Arc::new(persistence::MemoryStore::new()),
        platform_for(&base, true, Some(cache_path("hook")), false),
    )
    .await;
    let app = router(st.clone());
    *f.platform.lock().unwrap() = list(7, &[("ETHUSD", "trade")]);
    let body = json!({"event_id": "e", "tags": ["cms:platform-instruments"]}).to_string();
    let (s, b) = send(
        &app,
        "POST",
        "/internal/hooks/platform-instruments",
        Some(serde_json::from_str(&body).unwrap()),
        &[("x-signature", "sha256=00")],
    )
    .await;
    assert_eq!(
        (s, b["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("bad_signature"))
    );
    let sig = instruments::sign(body.as_bytes(), "whsec");
    let req = Request::builder()
        .method("POST")
        .uri("/internal/hooks/platform-instruments")
        .header("x-signature", sig)
        .body(Body::from(body.clone()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    for _ in 0..100 {
        if st.catalog.read().unwrap().version == Some(7) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(st.catalog.read().unwrap().platform, vec![(2, Mode::Trade)]);

    let quiet = state_on(
        Arc::new(persistence::MemoryStore::new()),
        Platform::new(None, base.clone(), None, Some(cache_path("hook-off"))),
    )
    .await;
    let (s, b) = send(
        &router(quiet),
        "POST",
        "/internal/hooks/platform-instruments",
        Some(json!({})),
        &[],
    )
    .await;
    assert_eq!(
        (s, b["error"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("webhook_disabled"))
    );
}

async fn wait_ops(f: &Fake, n: usize) -> Vec<Value> {
    for _ in 0..200 {
        let ops = f.ops.lock().unwrap().clone();
        if ops.len() >= n {
            return ops;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    f.ops.lock().unwrap().clone()
}

#[tokio::test]
async fn the_v2_stream_subscribes_by_delta_and_keeps_instruments_with_exposure() {
    let (f, base) = fake().await;
    *f.platform.lock().unwrap() = list(1, &[("EURUSD", "trade"), ("XAUUSD", "trade")]);
    let st = state_on(
        Arc::new(persistence::MemoryStore::new()),
        platform_for(&base, true, Some(cache_path("stream")), false),
    )
    .await;
    instruments::poll_once(&st).await.unwrap();
    feed_v2::spawn(st.clone());
    let ops = wait_ops(&f, 1).await;
    assert_eq!(ops[0]["op"], "subscribe");
    let mut subscribed: Vec<String> = ops[0]["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    subscribed.sort();
    assert_eq!(subscribed, vec!["EURUSD", "XAUUSD"]);

    let eur = id_of(&st, "EURUSD");
    for _ in 0..100 {
        let _ = f.push.send(
            json!({"type": "quote", "s": "EURUSD", "b": "1.08412", "a": "1.08415", "t": 1})
                .to_string(),
        );
        if st
            .tickers
            .lock()
            .await
            .get(&eur)
            .is_some_and(|t| t.bid == 108_412)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        st.tickers
            .lock()
            .await
            .get(&eur)
            .map(|t| (t.bid, t.ask, t.last)),
        Some((108_412, 108_415, 108_413))
    );
    assert!(st.market_of(eur).unwrap().tradable(crate::now_ms()));

    let app = router(st.clone());
    let (s, b) = send(
        &app,
        "POST",
        "/deals",
        Some(json!({"instrument": eur, "side": "buy", "qty": 1000})),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");

    *f.platform.lock().unwrap() = list(2, &[("BTCUSD", "trade")]);
    instruments::poll_once(&st).await.unwrap();
    let ops = wait_ops(&f, 2).await;
    assert_eq!(ops[1]["op"], "unsubscribe");
    assert_eq!(
        ops[1]["symbols"],
        json!(["XAUUSD"]),
        "EURUSD с открытой позицией не отписывается"
    );
    let _ = f.push.send(
        json!({"type": "status", "s": "EURUSD", "status": "closed", "nextOpen": 5, "t": 2})
            .to_string(),
    );
    for _ in 0..100 {
        if st.market_of(eur).map(|m| m.status) == Some(Status::Closed) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(st.market_of(eur).unwrap().next_open, Some(5));
}

#[test]
fn platform_list_parsing_follows_the_contract() {
    let ok = instruments::parse_platform_list(&json!({"version": 3, "instruments": [
        {"symbol": "btcusd", "mode": "trade"}, {"symbol": "BTCUSD", "mode": "closeOnly"}, {"symbol": "LTCUSD", "mode": "closeOnly"}]}))
    .unwrap();
    assert_eq!(ok.instruments.len(), 2);
    assert_eq!(ok.instruments[0].symbol, "BTCUSD");
    assert_eq!(ok.instruments[1].mode, Mode::CloseOnly);
    assert!(instruments::parse_platform_list(&json!({"version": 1, "instruments": []})).is_err());
    assert!(instruments::parse_platform_list(
        &json!({"version": 1, "instruments": [{"symbol": "X", "mode": "sell"}]})
    )
    .is_err());
    assert!(instruments::parse_platform_list(&json!({"instruments": []})).is_err());
}

#[test]
fn webhook_signature_is_hmac_sha256_hex() {
    let body = br#"{"tags":["x"]}"#;
    let sig = instruments::sign(body, "secret");
    assert!(sig.starts_with("sha256=") && sig.len() == 7 + 64);
    assert!(instruments::verify_signature(body, Some(&sig), "secret"));
    assert!(!instruments::verify_signature(body, Some(&sig), "other"));
    assert!(!instruments::verify_signature(b"{}", Some(&sig), "secret"));
    assert!(!instruments::verify_signature(body, None, "secret"));
    assert!(!instruments::verify_signature(
        body,
        Some("sha256=zz"),
        "secret"
    ));
}

#[test]
fn storage_precision_comes_from_the_catalog() {
    let (v1, skipped) = instruments::parse_exchange_info(&exchange_info());
    assert_eq!(skipped, vec!["NOLOTUSDT".to_string()]);
    let avax = v1.iter().find(|e| e.symbol == "AVAXUSD").unwrap();
    assert_eq!(
        instruments::storage_precision(avax),
        Some((3, 2)),
        "новая запись — точность биржи (ТД-002)"
    );
    let (v2, skipped) = instruments::parse_v2_instruments(&v2_catalog("0.00001"));
    assert!(skipped.is_empty());
    let p: Vec<(String, Option<(u8, u8)>)> = v2
        .iter()
        .map(|e| (e.symbol.clone(), instruments::storage_precision(e)))
        .collect();
    assert_eq!(
        p,
        vec![
            ("EURUSD".into(), Some((5, 0))),
            ("XAUUSD".into(), Some((2, 0))),
            ("AAPL".into(), Some((2, 0))),
        ]
    );
}

#[test]
fn the_v1_stream_url_lists_ticker_and_trades_of_the_wanted_pairs() {
    let set: BTreeSet<String> = ["ETHUSDT".to_string(), "BTCUSDT".to_string()].into();
    assert_eq!(
        feed::stream_url("wss://md.example/k", &set),
        "wss://md.example/k/stream?streams=btcusdt@ticker/btcusdt@aggTrade/ethusdt@ticker/ethusdt@aggTrade"
    );
}

#[tokio::test]
async fn the_legacy_index_still_resolves_both_spellings() {
    let st = build_state();
    let idx = st.symbol_index();
    assert_eq!(idx.get("BTCUSD"), Some(&1));
    assert_eq!(idx.get("BTCUSDT"), Some(&1));
    assert_eq!(idx.get("SHIBUSD"), Some(&28));
}
