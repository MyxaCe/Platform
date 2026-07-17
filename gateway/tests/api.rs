//! API-тесты шлюза через `oneshot` (без реальной сети).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn app() -> Router {
    let st = gateway::build_state();
    gateway::seed_demo(&st).await; // BTC-USDT (id 1), alice-token/bob-token с депозитами
    gateway::router(st)
}

/// Отправить запрос и вернуть (статус, тело-JSON | Null).
async fn send(app: &Router, method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(serde_json::to_vec(&v).unwrap())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let val = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::Null) };
    (status, val)
}

fn has_trade(events: &Value) -> bool {
    events.as_array().map(|a| a.iter().any(|e| e["type"] == "trade")).unwrap_or(false)
}

#[tokio::test]
async fn health_ok() {
    let (status, _) = send(&app().await, "GET", "/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn place_order_without_auth_is_401() {
    let body = json!({"instrument":1,"side":"buy","type":"limit","price":100,"qty":5});
    let (status, _) = send(&app().await, "POST", "/orders", None, Some(body)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn invalid_token_is_401() {
    let body = json!({"instrument":1,"side":"buy","type":"limit","price":100,"qty":5});
    let (status, _) = send(&app().await, "POST", "/orders", Some("nope"), Some(body)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn full_trade_flow_moves_balances() {
    let app = app().await;

    // Alice продаёт 5 BTC по 100.
    let (s1, _) = send(&app, "POST", "/orders", Some("alice-token"),
        Some(json!({"instrument":1,"side":"sell","type":"limit","price":100,"qty":5}))).await;
    assert_eq!(s1, StatusCode::OK);

    // Bob покупает 5 BTC по 100 → сделка.
    let (s2, body2) = send(&app, "POST", "/orders", Some("bob-token"),
        Some(json!({"instrument":1,"side":"buy","type":"limit","price":100,"qty":5}))).await;
    assert_eq!(s2, StatusCode::OK);
    assert!(has_trade(&body2["events"]), "ожидалась сделка в событиях: {body2}");

    // Bob получил +5 BTC (asset 1): 1_000_000 -> 1_000_005.
    let (s3, bal) = send(&app, "GET", "/balance/1", Some("bob-token"), None).await;
    assert_eq!(s3, StatusCode::OK);
    assert_eq!(bal["available"], 1_000_005);
    assert_eq!(bal["held"], 0);
}

#[tokio::test]
async fn book_snapshot_reflects_resting_order() {
    let app = app().await;
    send(&app, "POST", "/orders", Some("alice-token"),
        Some(json!({"instrument":1,"side":"sell","type":"limit","price":100,"qty":5}))).await;

    let (status, book) = send(&app, "GET", "/book/1?depth=10", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(book["asks"][0]["price"], 100);
    assert_eq!(book["asks"][0]["qty"], 5);
    assert!(book["bids"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn self_trade_is_409() {
    let app = app().await;
    send(&app, "POST", "/orders", Some("alice-token"),
        Some(json!({"instrument":1,"side":"sell","type":"limit","price":100,"qty":5}))).await;
    let (status, _) = send(&app, "POST", "/orders", Some("alice-token"),
        Some(json!({"instrument":1,"side":"buy","type":"limit","price":100,"qty":5}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn market_buy_via_api() {
    let app = app().await;
    send(&app, "POST", "/orders", Some("alice-token"),
        Some(json!({"instrument":1,"side":"sell","type":"limit","price":100,"qty":5}))).await;
    let (status, body) = send(&app, "POST", "/orders", Some("bob-token"),
        Some(json!({"instrument":1,"side":"buy","type":"market","qty":3}))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_trade(&body["events"]));
}
