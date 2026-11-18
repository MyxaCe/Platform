use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

fn app() -> Router {
    gateway::router(gateway::build_state())
}

async fn send(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
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
    let val = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, val)
}

#[tokio::test]
async fn health_ok() {
    let (status, _) = send(&app(), "GET", "/health", None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn account_is_absent_until_the_first_operation() {
    let (status, acc) = send(&app(), "GET", "/account", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(acc["opened"], json!(false), "{acc}");
    assert!(acc["balance"].is_null(), "остаток не выдумывается: {acc}");
    assert!(acc["equity"].is_null(), "{acc}");
    assert!(acc["used_margin"].is_null(), "{acc}");
}

#[tokio::test]
async fn open_deal_without_price_is_503() {
    let body = json!({ "instrument": 1, "side": "buy", "qty": 1 });
    let (status, _) = send(&app(), "POST", "/deals", Some(body)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn open_deal_unknown_instrument_is_404() {
    let body = json!({ "instrument": 9999, "side": "buy", "qty": 1 });
    let (status, _) = send(&app(), "POST", "/deals", Some(body)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn open_deal_bad_side_is_400() {
    let body = json!({ "instrument": 1, "side": "sideways", "qty": 1 });
    let (status, _) = send(&app(), "POST", "/deals", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pending_place_and_list() {
    let app = app();
    let body = json!({ "instrument": 1, "side": "buy", "qty": 1000, "price": 5_000_000 });
    let (status, _) = send(&app, "POST", "/pending", Some(body)).await;
    assert_eq!(status, StatusCode::OK);

    let (status, list) = send(&app, "GET", "/pending", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
}

struct FailingStore;

#[async_trait::async_trait]
impl gateway::persistence::Persistence for FailingStore {
    async fn init(&self) -> Result<(), gateway::persistence::StoreError> {
        Ok(())
    }
    async fn load_all(
        &self,
    ) -> Result<gateway::persistence::LoadedState, gateway::persistence::StoreError> {
        Ok(gateway::persistence::LoadedState::default())
    }
    async fn save_account(
        &self,
        _u: domain::account::UserId,
        _s: &broker::AccountSnapshot,
        _e: &[broker::journal::AccountEvent],
        _out: &[gateway::persistence::OutboxRecord],
    ) -> Result<(), gateway::persistence::StoreError> {
        Err(gateway::persistence::StoreError("disk on fire".into()))
    }

    async fn claim_outbox(
        &self,
        _limit: i64,
        _lease_ms: i64,
    ) -> Result<Vec<gateway::persistence::OutboxClaim>, gateway::persistence::StoreError> {
        Err(gateway::persistence::StoreError("disk on fire".into()))
    }
    async fn mark_published(&self, _id: i64) -> Result<(), gateway::persistence::StoreError> {
        Err(gateway::persistence::StoreError("disk on fire".into()))
    }
    async fn mark_failed(
        &self,
        _id: i64,
        _attempts: i32,
        _retry_in_ms: i64,
        _error: &str,
    ) -> Result<(), gateway::persistence::StoreError> {
        Err(gateway::persistence::StoreError("disk on fire".into()))
    }
    async fn outbox_backlog(&self) -> Result<i64, gateway::persistence::StoreError> {
        Err(gateway::persistence::StoreError("disk on fire".into()))
    }
    async fn identity_of(
        &self,
        _u: domain::account::UserId,
    ) -> Result<Option<(String, String)>, gateway::persistence::StoreError> {
        Err(gateway::persistence::StoreError("disk on fire".into()))
    }
}

#[tokio::test]
async fn storage_failure_rolls_back_and_returns_503() {
    let st = gateway::build_state_with(std::sync::Arc::new(FailingStore));
    let app = gateway::router(st);

    let body = json!({ "instrument": 1, "side": "buy", "qty": 1000, "price": 5_000_000 });
    let (status, _) = send(&app, "POST", "/pending", Some(body)).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "запись не прошла → 503"
    );

    let (status, list) = send(&app, "GET", "/pending", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        list.as_array().unwrap().len(),
        0,
        "ордер откачен, в памяти его нет"
    );
}
