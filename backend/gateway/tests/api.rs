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

// ---- Персистентность (ADR-016) --------------------------------------------

/// Хранилище, которое всегда падает на записи — проверяем дисциплину отката.
struct FailingStore;

#[async_trait::async_trait]
impl gateway::persistence::Persistence for FailingStore {
    async fn init(&self) -> Result<(), gateway::persistence::StoreError> {
        Ok(())
    }
    async fn load_all(&self) -> Result<gateway::persistence::LoadedState, gateway::persistence::StoreError> {
        Ok(gateway::persistence::LoadedState::default())
    }
    async fn save_account(&self, _u: domain::account::UserId, _s: &broker::AccountSnapshot) -> Result<(), gateway::persistence::StoreError> {
        Err(gateway::persistence::StoreError("disk on fire".into()))
    }
    async fn save_user(&self, _u: domain::account::UserId, _t: &str) -> Result<(), gateway::persistence::StoreError> {
        Ok(())
    }
}

/// Если запись в хранилище не прошла — клиент получает 503, а состояние счёта
/// откатывается: ордера, которого нет на диске, не должно быть и в памяти.
#[tokio::test]
async fn storage_failure_rolls_back_and_returns_503() {
    let st = gateway::build_state_with(std::sync::Arc::new(FailingStore));
    gateway::seed_demo(&st).await;
    let app = gateway::router(st);

    let body = json!({ "instrument": 1, "side": "buy", "qty": 1000, "price": 5_000_000 });
    let (status, _) = send(&app, "POST", "/pending", Some("alice-token"), Some(body)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "запись не прошла → 503");

    let (status, list) = send(&app, "GET", "/pending", Some("alice-token"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 0, "ордер откачен, в памяти его нет");
}

/// С NoopStore поведение прежнее: ордер размещается и виден.
#[tokio::test]
async fn noop_store_keeps_previous_behaviour() {
    let app = app().await;
    let body = json!({ "instrument": 1, "side": "buy", "qty": 1000, "price": 5_000_000 });
    let (status, _) = send(&app, "POST", "/pending", Some("alice-token"), Some(body)).await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = send(&app, "GET", "/pending", Some("alice-token"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
}

// ---- Аутентификация (ADR-018) ---------------------------------------------

/// Запрос с cookie; возвращает (статус, тело, значение Set-Cookie).
async fn send_ck(app: &Router, method: &str, uri: &str, ck: Option<&str>, body: Option<Value>) -> (StatusCode, Value, Option<String>) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(c) = ck {
        b = b.header("cookie", c);
    }
    let req = match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(serde_json::to_vec(&v).unwrap())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let set = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let val = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::Null) };
    (status, val, set)
}

/// Вытащить `session=...` из Set-Cookie, чтобы отправить обратно.
fn session_of(set: &Option<String>) -> String {
    set.as_ref().unwrap().split(';').next().unwrap().to_string()
}

const CREDS: fn(&str) -> Value = |email| json!({ "email": email, "password": "Str0ngPass1" });

#[tokio::test]
async fn register_creates_account_and_opens_session() {
    let app = app().await;
    let (status, body, set) = send_ck(&app, "POST", "/auth/register", None, Some(CREDS("Trader@Example.com"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["email"], "trader@example.com", "почта приводится к нижнему регистру");
    let ck = set.expect("должна выдаваться сессионная cookie");
    assert!(ck.contains("HttpOnly"), "cookie обязана быть HttpOnly, иначе её достанет XSS");
    assert!(ck.contains("SameSite=Lax"));
    assert!(!ck.contains("Str0ngPass1"), "в cookie не должно быть ничего от пароля");
}

#[tokio::test]
async fn register_rejects_weak_password_and_bad_email() {
    let app = app().await;
    let (s1, _, _) = send_ck(&app, "POST", "/auth/register", None, Some(json!({ "email": "a@b.com", "password": "short1" }))).await;
    assert_eq!(s1, StatusCode::BAD_REQUEST);
    let (s2, _, _) = send_ck(&app, "POST", "/auth/register", None, Some(json!({ "email": "a@b.com", "password": "onlyletters" }))).await;
    assert_eq!(s2, StatusCode::BAD_REQUEST, "пароль без цифры не проходит и на сервере");
    let (s3, _, _) = send_ck(&app, "POST", "/auth/register", None, Some(CREDS("not-an-email"))).await;
    assert_eq!(s3, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn register_rejects_duplicate_email() {
    let app = app().await;
    let (s1, _, _) = send_ck(&app, "POST", "/auth/register", None, Some(CREDS("dup@example.com"))).await;
    assert_eq!(s1, StatusCode::OK);
    let (s2, _, _) = send_ck(&app, "POST", "/auth/register", None, Some(CREDS("DUP@example.com"))).await;
    assert_eq!(s2, StatusCode::CONFLICT, "регистр почты не должен позволять завести второй аккаунт");
}

#[tokio::test]
async fn login_works_and_wrong_password_is_rejected() {
    let app = app().await;
    send_ck(&app, "POST", "/auth/register", None, Some(CREDS("login@example.com"))).await;

    let (ok, body, set) = send_ck(&app, "POST", "/auth/login", None, Some(CREDS("login@example.com"))).await;
    assert_eq!(ok, StatusCode::OK);
    assert_eq!(body["email"], "login@example.com");
    assert!(set.is_some());

    let (bad, b1, _) = send_ck(&app, "POST", "/auth/login", None, Some(json!({ "email": "login@example.com", "password": "WrongPass1" }))).await;
    assert_eq!(bad, StatusCode::UNAUTHORIZED);

    // Незарегистрированная почта отвечает тем же статусом и тем же текстом:
    // иначе форма входа превращается в проверялку «есть ли такой клиент».
    let (unknown, b2, _) = send_ck(&app, "POST", "/auth/login", None, Some(CREDS("nobody@example.com"))).await;
    assert_eq!(unknown, StatusCode::UNAUTHORIZED);
    assert_eq!(b1["error"], b2["error"], "ответы должны быть неразличимы");
}

#[tokio::test]
async fn session_authenticates_and_logout_kills_it() {
    let app = app().await;
    let (_, _, set) = send_ck(&app, "POST", "/auth/register", None, Some(CREDS("me@example.com"))).await;
    let ck = session_of(&set);

    let (s, body, _) = send_ck(&app, "GET", "/auth/me", Some(&ck), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["email"], "me@example.com");

    // Сессия годится и для торговых эндпоинтов, не только для /auth/me.
    let (s, _, _) = send_ck(&app, "GET", "/pending", Some(&ck), None).await;
    assert_eq!(s, StatusCode::OK);

    let (s, _, cleared) = send_ck(&app, "POST", "/auth/logout", Some(&ck), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(cleared.unwrap().contains("Max-Age=0"), "cookie должна гаситься");

    let (s, _, _) = send_ck(&app, "GET", "/auth/me", Some(&ck), None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "после выхода сессия недействительна");
}

#[tokio::test]
async fn no_session_no_access() {
    let app = app().await;
    let (s, _, _) = send_ck(&app, "GET", "/auth/me", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, _) = send_ck(&app, "GET", "/auth/me", Some("session=made-up-token"), None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_attempts_are_rate_limited() {
    let app = app().await;
    send_ck(&app, "POST", "/auth/register", None, Some(CREDS("brute@example.com"))).await;
    let wrong = json!({ "email": "brute@example.com", "password": "WrongPass1" });
    let mut saw_429 = false;
    for _ in 0..14 {
        let (s, _, _) = send_ck(&app, "POST", "/auth/login", None, Some(wrong.clone())).await;
        if s == StatusCode::TOO_MANY_REQUESTS {
            saw_429 = true;
            break;
        }
    }
    assert!(saw_429, "перебор паролей должен упираться в ограничитель");
}
