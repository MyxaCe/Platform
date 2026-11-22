use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use broker::journal::{EventKind, Operator, Origin};
use broker::Cents;
use domain::account::{AccountKind, UserId};

use super::{
    account_lock, bus, commit_journal, ensure_account_opened, journal, marks, now_ms,
    trading_frozen, with_account, with_account_gated, AppState, Gate,
};

pub const TOKEN_HEADER: &str = "x-internal-token";
const REF_PREFIX: &str = "ext:";
const MAX_REF_LEN: usize = 200;
const MIN_TOKEN_LEN: usize = 32;
const CURRENCY: &str = "USD";

pub(crate) fn token_from_env(raw: Option<&str>) -> Option<Arc<str>> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => {
            println!(
                "[money] INTERNAL_MONEY_TOKEN не задан — денежные ручки /internal ВЫКЛЮЧЕНЫ (404)"
            );
            None
        }
        Some(t) => {
            if t.len() < MIN_TOKEN_LEN {
                eprintln!(
                    "[money] ВНИМАНИЕ: INTERNAL_MONEY_TOKEN короче {MIN_TOKEN_LEN} символов — секрет слабый"
                );
            }
            println!("[money] денежные ручки /internal включены");
            Some(Arc::from(t))
        }
    }
}

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/internal/sites/:tenant/users/:sub/real/deposit",
            post(deposit),
        )
        .route(
            "/internal/sites/:tenant/users/:sub/real/withdraw",
            post(withdraw),
        )
        .route(
            "/internal/sites/:tenant/users/:sub/:kind/trading-freeze",
            post(trading_freeze),
        )
        .route(
            "/internal/sites/:tenant/users/:sub/:kind/leverage",
            post(set_leverage),
        )
        .route(
            "/internal/sites/:tenant/users/:sub/:kind/fees",
            post(set_fees),
        )
        .route("/internal/sites/:tenant/users/:sub/accounts", get(accounts))
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let n = a.len().max(b.len());
    let mut diff = (a.len() ^ b.len()) as u64;
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= u64::from(x ^ y);
    }
    std::hint::black_box(diff) == 0
}

fn fail(code: StatusCode, error: &str, detail: Option<String>) -> Response {
    let mut body = serde_json::json!({ "error": error });
    if let Some(d) = detail {
        body["detail"] = serde_json::Value::from(d);
    }
    (code, Json(body)).into_response()
}

pub(crate) fn authorize(st: &AppState, headers: &HeaderMap) -> Result<(), Box<Response>> {
    let Some(expected) = st.internal_money_token.as_deref() else {
        return Err(Box::new(StatusCode::NOT_FOUND.into_response()));
    };
    let Some(got) = headers.get(TOKEN_HEADER) else {
        return Err(Box::new(fail(
            StatusCode::UNAUTHORIZED,
            "missing_internal_token",
            None,
        )));
    };
    if !ct_eq(got.as_bytes(), expected.as_bytes()) {
        return Err(Box::new(fail(
            StatusCode::FORBIDDEN,
            "invalid_internal_token",
            None,
        )));
    }
    Ok(())
}

#[derive(Deserialize)]
struct MoneyReq {
    amount_minor: i64,
    currency: String,
    external_ref: String,
    #[serde(alias = "operator")]
    initiator: String,
    reason: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Deposit,
    Withdraw,
}

impl Op {
    fn tag(self) -> &'static str {
        match self {
            Op::Deposit => "deposit",
            Op::Withdraw => "withdraw",
        }
    }
}

struct Valid {
    amount: Cents,
    external_ref: String,
    operator: Operator,
}

fn validate(req: MoneyReq) -> Result<Valid, Box<Response>> {
    let bad = |e: &str, d: &str| Box::new(fail(StatusCode::BAD_REQUEST, e, Some(d.to_string())));
    if req.amount_minor <= 0 {
        return Err(bad(
            "invalid_amount",
            "amount_minor — целое число минимальных единиц больше нуля",
        ));
    }
    if !req.currency.trim().eq_ignore_ascii_case(CURRENCY) {
        return Err(bad(
            "unsupported_currency",
            "валюта счёта терминала — USD; пересчёт из актива депозита — на стороне сайта",
        ));
    }
    let external_ref = req.external_ref.trim().to_string();
    if external_ref.is_empty() || external_ref.len() > MAX_REF_LEN {
        return Err(bad(
            "invalid_external_ref",
            "external_ref обязателен, не длиннее 200 символов",
        ));
    }
    if req.initiator.trim().is_empty() {
        return Err(bad("missing_initiator", "initiator обязателен"));
    }
    if req.reason.trim().is_empty() {
        return Err(bad("missing_reason", "reason обязателен"));
    }
    let operator = Operator::new(
        &req.initiator,
        &req.reason,
        &format!("{REF_PREFIX}{external_ref}"),
    )
    .map_err(|e| bad("bad_request", &e.to_string()))?;
    Ok(Valid {
        amount: Cents::from(req.amount_minor),
        external_ref,
        operator,
    })
}

async fn find_ref(st: &AppState, external_ref: &str) -> Option<(UserId, Op, Cents, u64)> {
    let incident = format!("{REF_PREFIX}{external_ref}");
    let js = st.journals.lock().await;
    for (user, j) in js.iter() {
        for e in j.events() {
            let Some(op) = e.origin().operator() else {
                continue;
            };
            if op.incident() != incident {
                continue;
            }
            match e.kind() {
                EventKind::Deposited { amount } => {
                    return Some((*user, Op::Deposit, *amount, e.seq()))
                }
                EventKind::Withdrawn { amount } => {
                    return Some((*user, Op::Withdraw, *amount, e.seq()))
                }
                _ => {}
            }
        }
    }
    None
}

async fn deposit(
    State(st): State<AppState>,
    Path((tenant, sub)): Path<(String, String)>,
    headers: HeaderMap,
    body: Result<Json<MoneyReq>, JsonRejection>,
) -> Response {
    run(st, Op::Deposit, tenant, sub, headers, body).await
}

async fn withdraw(
    State(st): State<AppState>,
    Path((tenant, sub)): Path<(String, String)>,
    headers: HeaderMap,
    body: Result<Json<MoneyReq>, JsonRejection>,
) -> Response {
    run(st, Op::Withdraw, tenant, sub, headers, body).await
}

async fn run(
    st: AppState,
    op: Op,
    tenant: String,
    sub: String,
    headers: HeaderMap,
    body: Result<Json<MoneyReq>, JsonRejection>,
) -> Response {
    if let Err(r) = authorize(&st, &headers) {
        return *r;
    }
    let req = match body {
        Ok(Json(r)) => r,
        Err(rej) => {
            return fail(
                StatusCode::BAD_REQUEST,
                "bad_request",
                Some(rej.body_text()),
            )
        }
    };
    let v = match validate(req) {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let (tenant, sub) = (tenant.trim().to_string(), sub.trim().to_string());
    if tenant.is_empty() || sub.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "bad_account", None);
    }

    let found = match op {
        Op::Deposit => st
            .store
            .resolve_account(&tenant, &sub, AccountKind::Real)
            .await
            .map(Some),
        Op::Withdraw => {
            st.store
                .find_account(&tenant, &sub, AccountKind::Real)
                .await
        }
    };
    let user = match found {
        Ok(Some(u)) => u,
        Ok(None) => return fail(StatusCode::NOT_FOUND, "account_not_opened", None),
        Err(e) => {
            eprintln!("[money] реальный счёт {tenant}/{sub} не разрешён: {e}");
            return fail(
                StatusCode::SERVICE_UNAVAILABLE,
                "identity_store_unavailable",
                None,
            );
        }
    };

    let _money = st.money_lock.lock().await;

    if let Some((u, prev_op, amount, seq)) = find_ref(&st, &v.external_ref).await {
        if u == user && prev_op == op && amount == v.amount {
            println!(
                "[money] повтор {} ref={} по счёту {} — проводка уже есть (seq {seq}), второй не будет",
                op.tag(),
                v.external_ref,
                user.0
            );
            return ok(&st, op, &tenant, &sub, user, &v, seq, true).await;
        }
        return fail(
            StatusCode::CONFLICT,
            "external_ref_conflict",
            Some(format!(
                "external_ref уже использован: {} {} по другому счёту или с другой суммой",
                prev_op.tag(),
                amount
            )),
        );
    }

    let origin = Origin::Operator(v.operator.clone());
    let amount = v.amount;
    let done = match op {
        Op::Deposit => with_account(&st, user, origin, |b| Ok(b.deposit(user, amount)))
            .await
            .map_err(IntoResponse::into_response),
        Op::Withdraw => {
            if !st.broker.lock().await.has_account(user) {
                return fail(StatusCode::NOT_FOUND, "account_not_opened", None);
            }
            let m = marks(&st).await;
            let mut free_seen: Option<Cents> = None;
            let r = with_account_gated(&st, user, origin, Gate::NotFrozen, |b| {
                b.withdraw(user, amount, &m).map_err(|e| match e {
                    broker::BrokerError::InsufficientFunds => {
                        free_seen = b.free_margin(user, &m);
                        super::err(StatusCode::CONFLICT, "insufficient_free_margin")
                    }
                    other => super::deal_error(other),
                })
            })
            .await;
            match (r, free_seen) {
                (Ok(b), _) => Ok(b),
                (Err(_), Some(free)) => {
                    let body = serde_json::json!({
                        "error": "insufficient_free_margin",
                        "detail": "сумма вывода больше свободной маржи (equity минус маржа открытых позиций)",
                        "requested_minor": amount,
                        "free_margin_minor": free,
                        "currency": CURRENCY,
                    });
                    return (StatusCode::CONFLICT, Json(body)).into_response();
                }
                (Err(e), None) => Err(e.into_response()),
            }
        }
    };
    if let Err(r) = done {
        return r;
    }

    let seq = match find_ref(&st, &v.external_ref).await {
        Some((_, _, _, seq)) => seq,
        None => {
            return fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "journal_not_committed",
                None,
            )
        }
    };
    println!(
        "[money] {} {} {} по счёту {} ({tenant}/{sub}), ref={}, seq {seq}",
        op.tag(),
        amount,
        CURRENCY,
        user.0,
        v.external_ref
    );
    ok(&st, op, &tenant, &sub, user, &v, seq, false).await
}

#[allow(clippy::too_many_arguments)]
async fn ok(
    st: &AppState,
    op: Op,
    tenant: &str,
    sub: &str,
    user: UserId,
    v: &Valid,
    seq: u64,
    replayed: bool,
) -> Response {
    let m = marks(st).await;
    let b = st.broker.lock().await;
    let body = serde_json::json!({
        "operation": op.tag(),
        "tenant": tenant,
        "sub": sub,
        "account_kind": "real",
        "account_id": bus::account_uuid(tenant, sub, AccountKind::Real),
        "external_ref": v.external_ref,
        "amount_minor": v.amount,
        "currency": CURRENCY,
        "balance_minor": b.balance(user),
        "free_margin_minor": b.free_margin(user, &m),
        "seq": seq,
        "replayed": replayed,
    });
    (StatusCode::OK, Json(body)).into_response()
}

#[derive(Deserialize)]
struct FreezeReq {
    frozen: bool,
    reason: String,
    #[serde(alias = "initiator")]
    operator: String,
    #[serde(default)]
    external_ref: Option<String>,
}

async fn trading_freeze(
    State(st): State<AppState>,
    Path((tenant, sub, kind)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Result<Json<FreezeReq>, JsonRejection>,
) -> Response {
    if let Err(r) = authorize(&st, &headers) {
        return *r;
    }
    let Some(kind) = AccountKind::parse(&kind) else {
        return fail(StatusCode::NOT_FOUND, "unknown_account_kind", None);
    };
    let req = match body {
        Ok(Json(r)) => r,
        Err(rej) => {
            return fail(
                StatusCode::BAD_REQUEST,
                "bad_request",
                Some(rej.body_text()),
            )
        }
    };
    if req.operator.trim().is_empty() {
        return fail(StatusCode::BAD_REQUEST, "missing_operator", None);
    }
    if req.reason.trim().is_empty() {
        return fail(StatusCode::BAD_REQUEST, "missing_reason", None);
    }
    let incident = match req.external_ref.as_deref().map(str::trim) {
        Some(r) if r.len() > MAX_REF_LEN => {
            return fail(StatusCode::BAD_REQUEST, "invalid_external_ref", None)
        }
        Some(r) if !r.is_empty() => format!("{REF_PREFIX}{r}"),
        _ => "trading-freeze".to_string(),
    };
    let operator = match Operator::new(&req.operator, &req.reason, &incident) {
        Ok(o) => o,
        Err(e) => return fail(StatusCode::BAD_REQUEST, "bad_request", Some(e.to_string())),
    };
    let (tenant, sub) = (tenant.trim().to_string(), sub.trim().to_string());
    if tenant.is_empty() || sub.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "bad_account", None);
    }
    let user = match st.store.resolve_account(&tenant, &sub, kind).await {
        Ok(u) => u,
        Err(e) => {
            eprintln!(
                "[money] счёт {tenant}/{sub}/{} не разрешён: {e}",
                kind.tag()
            );
            return fail(
                StatusCode::SERVICE_UNAVAILABLE,
                "identity_store_unavailable",
                None,
            );
        }
    };

    let _money = st.money_lock.lock().await;
    if st.quarantined.lock().await.contains(&user) {
        return fail(StatusCode::LOCKED, "account_quarantined", None);
    }
    let lock = account_lock(&st, user).await;
    let _guard = lock.lock().await;
    if let Err(e) = ensure_account_opened(&st, user).await {
        return e.into_response();
    }

    let last_seq = |js: &std::collections::HashMap<UserId, journal::Journal>| {
        js.get(&user)
            .and_then(|j| j.events().last())
            .map(|e| e.seq())
            .unwrap_or(0)
    };
    if trading_frozen(&st, user).await == req.frozen {
        let seq = last_seq(&*st.journals.lock().await);
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "tenant": tenant, "sub": sub, "account_kind": kind.tag(),
                "account_id": bus::account_uuid(&tenant, &sub, kind),
                "frozen": req.frozen, "changed": false, "seq": seq,
            })),
        )
            .into_response();
    }

    let (snap, start_balance) = {
        let b = st.broker.lock().await;
        (b.snapshot(user), b.start_balance())
    };
    let sealed = {
        let mut js = st.journals.lock().await;
        let j = js
            .entry(user)
            .or_insert_with(|| journal::Journal::new(start_balance));
        let sealed = j.seal(
            now_ms(),
            &Origin::Operator(operator),
            &[EventKind::TradingFreezeSet { frozen: req.frozen }],
        );
        let diverged = journal::reconcile(&j.preview(&sealed), &snap);
        if !diverged.is_empty() {
            for d in &diverged {
                eprintln!("[money] заморозка счёта {} не сходится: {d}", user.0);
            }
            return fail(StatusCode::UNPROCESSABLE_ENTITY, "journal_divergence", None);
        }
        sealed
    };
    if let Err(e) = st.store.save_account(user, &snap, &sealed, &[]).await {
        eprintln!("[money] заморозка счёта {} не записана: {e}", user.0);
        return fail(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", None);
    }
    commit_journal(&st, user, sealed).await;
    let seq = last_seq(&*st.journals.lock().await);
    println!(
        "[money] торговля по счёту {} ({tenant}/{sub}) {} оператором {}: {}",
        user.0,
        if req.frozen {
            "ЗАМОРОЖЕНА"
        } else {
            "разморожена"
        },
        req.operator.trim(),
        req.reason.trim()
    );
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "tenant": tenant, "sub": sub, "account_kind": kind.tag(),
            "account_id": bus::account_uuid(&tenant, &sub, kind),
            "frozen": req.frozen, "changed": true, "seq": seq,
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
struct LeverageReq {
    leverage: i64,
    #[serde(default)]
    leverage_base: Option<i64>,
    #[serde(default)]
    leverage_classes: Option<std::collections::BTreeMap<String, i64>>,
    reason: String,
    #[serde(alias = "initiator")]
    operator: String,
    #[serde(default)]
    external_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LeverageTable {
    pub base: i128,
    pub classes: broker::ClassLeverage,
}

pub(crate) fn leverage_table(
    base: Option<i64>,
    classes: Option<&std::collections::BTreeMap<String, i64>>,
) -> Result<Option<LeverageTable>, &'static str> {
    let (Some(base), Some(classes)) = (base, classes) else {
        return Ok(None);
    };
    let in_range = |l: i64| (1..=broker::MAX_LEVERAGE).contains(&i128::from(l));
    if !in_range(base) {
        return Err("invalid_leverage");
    }
    let mut out = broker::ClassLeverage::new();
    for (k, v) in classes {
        let class = broker::LevClass::parse(k.trim()).ok_or("invalid_leverage_class")?;
        if !in_range(*v) {
            return Err("invalid_leverage");
        }
        out.insert(class, i128::from(*v));
    }
    Ok(Some(LeverageTable {
        base: i128::from(base),
        classes: out,
    }))
}

pub(crate) struct LeverageApplied {
    pub previous: i128,
    pub previous_classes: broker::ClassLeverage,
    pub changed: bool,
}

pub(crate) async fn apply_leverage(
    st: &AppState,
    user: UserId,
    lev: i128,
    table: Option<LeverageTable>,
    operator: Operator,
) -> Result<LeverageApplied, super::ApiErr> {
    let (base, classes) = match table.filter(|_| st.leverage_classes_enabled) {
        Some(t) => (t.base, t.classes),
        None => (lev, broker::ClassLeverage::new()),
    };
    with_account(st, user, Origin::Operator(operator), |b| {
        let previous = b.account_leverage(user);
        let previous_classes = b.account_class_leverage(user);
        let base_changed = b
            .set_account_leverage(user, base)
            .map_err(super::deal_error)?;
        let classes_changed = b
            .set_account_class_leverage(user, classes)
            .map_err(super::deal_error)?;
        Ok(LeverageApplied {
            previous,
            previous_classes,
            changed: base_changed || classes_changed,
        })
    })
    .await
}

async fn set_leverage(
    State(st): State<AppState>,
    Path((tenant, sub, kind)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Result<Json<LeverageReq>, JsonRejection>,
) -> Response {
    if let Err(r) = authorize(&st, &headers) {
        return *r;
    }
    let Some(kind) = AccountKind::parse(&kind) else {
        return fail(StatusCode::NOT_FOUND, "unknown_account_kind", None);
    };
    let req = match body {
        Ok(Json(r)) => r,
        Err(rej) => {
            return fail(
                StatusCode::BAD_REQUEST,
                "bad_request",
                Some(rej.body_text()),
            )
        }
    };
    let lev = i128::from(req.leverage);
    if !(1..=broker::MAX_LEVERAGE).contains(&lev) {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_leverage",
            Some(format!("leverage — целое от 1 до {}", broker::MAX_LEVERAGE)),
        );
    }
    let table = if st.leverage_classes_enabled {
        match leverage_table(req.leverage_base, req.leverage_classes.as_ref()) {
            Ok(t) => t,
            Err(code) => return fail(StatusCode::BAD_REQUEST, code, None),
        }
    } else {
        None
    };
    if req.operator.trim().is_empty() {
        return fail(StatusCode::BAD_REQUEST, "missing_operator", None);
    }
    if req.reason.trim().is_empty() {
        return fail(StatusCode::BAD_REQUEST, "missing_reason", None);
    }
    let incident = match req.external_ref.as_deref().map(str::trim) {
        Some(r) if r.len() > MAX_REF_LEN => {
            return fail(StatusCode::BAD_REQUEST, "invalid_external_ref", None)
        }
        Some(r) if !r.is_empty() => format!("{REF_PREFIX}{r}"),
        _ => "leverage".to_string(),
    };
    let operator = match Operator::new(&req.operator, &req.reason, &incident) {
        Ok(o) => o,
        Err(e) => return fail(StatusCode::BAD_REQUEST, "bad_request", Some(e.to_string())),
    };
    let (tenant, sub) = (tenant.trim().to_string(), sub.trim().to_string());
    if tenant.is_empty() || sub.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "bad_account", None);
    }
    let user = match st.store.find_account(&tenant, &sub, kind).await {
        Ok(Some(u)) => u,
        Ok(None) => return fail(StatusCode::NOT_FOUND, "account_not_opened", None),
        Err(e) => {
            eprintln!(
                "[money] счёт {tenant}/{sub}/{} не разрешён: {e}",
                kind.tag()
            );
            return fail(
                StatusCode::SERVICE_UNAVAILABLE,
                "identity_store_unavailable",
                None,
            );
        }
    };

    let applied = match apply_leverage(&st, user, lev, table, operator).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let (previous, changed) = (applied.previous, applied.changed);
    let seq = st
        .journals
        .lock()
        .await
        .get(&user)
        .and_then(|j| j.events().last())
        .map(|e| e.seq())
        .unwrap_or(0);
    let (now, now_classes) = {
        let b = st.broker.lock().await;
        (b.account_leverage(user), b.account_class_leverage(user))
    };
    if changed {
        println!(
            "[money] плечо счёта {} ({tenant}/{sub}/{}) 1:{previous} → 1:{now} {:?} оператором {}: {}",
            user.0,
            kind.tag(),
            super::classes_dto(&now_classes),
            req.operator.trim(),
            req.reason.trim()
        );
    }
    let mut body = serde_json::json!({
        "tenant": tenant, "sub": sub, "account_kind": kind.tag(),
        "account_id": bus::account_uuid(&tenant, &sub, kind),
        "leverage": lev, "previous_leverage": previous,
        "changed": changed, "seq": seq,
    });
    if st.leverage_classes_enabled {
        body["leverage_base"] = serde_json::json!(now);
        body["leverage_classes"] = serde_json::json!(super::classes_dto(&now_classes));
        body["previous_leverage_classes"] =
            serde_json::json!(super::classes_dto(&applied.previous_classes));
    }
    (StatusCode::OK, Json(body)).into_response()
}

#[derive(Deserialize)]
struct FeesReq {
    markup_bps: i64,
    commission_bps: i64,
    reason: String,
    #[serde(alias = "initiator")]
    operator: String,
    #[serde(default)]
    external_ref: Option<String>,
}

pub(crate) async fn apply_fees(
    st: &AppState,
    user: UserId,
    fees: broker::Fees,
    operator: Operator,
) -> Result<(broker::Fees, bool), super::ApiErr> {
    with_account(st, user, Origin::Operator(operator), |b| {
        let before = b.account_fees(user);
        b.set_account_fees(user, fees)
            .map(|changed| (before, changed))
            .map_err(super::deal_error)
    })
    .await
}

async fn set_fees(
    State(st): State<AppState>,
    Path((tenant, sub, kind)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Result<Json<FeesReq>, JsonRejection>,
) -> Response {
    if let Err(r) = authorize(&st, &headers) {
        return *r;
    }
    let Some(kind) = AccountKind::parse(&kind) else {
        return fail(StatusCode::NOT_FOUND, "unknown_account_kind", None);
    };
    let req = match body {
        Ok(Json(r)) => r,
        Err(rej) => {
            return fail(
                StatusCode::BAD_REQUEST,
                "bad_request",
                Some(rej.body_text()),
            )
        }
    };
    let fees = broker::Fees {
        markup_bps: req.markup_bps,
        commission_bps: req.commission_bps,
    };
    if fees.validate().is_err() {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_fees",
            Some(format!(
                "markup_bps и commission_bps — целые от 0 до {}",
                broker::MAX_FEE_BPS
            )),
        );
    }
    if req.operator.trim().is_empty() {
        return fail(StatusCode::BAD_REQUEST, "missing_operator", None);
    }
    if req.reason.trim().is_empty() {
        return fail(StatusCode::BAD_REQUEST, "missing_reason", None);
    }
    let incident = match req.external_ref.as_deref().map(str::trim) {
        Some(r) if r.len() > MAX_REF_LEN => {
            return fail(StatusCode::BAD_REQUEST, "invalid_external_ref", None)
        }
        Some(r) if !r.is_empty() => format!("{REF_PREFIX}{r}"),
        _ => "fees".to_string(),
    };
    let operator = match Operator::new(&req.operator, &req.reason, &incident) {
        Ok(o) => o,
        Err(e) => return fail(StatusCode::BAD_REQUEST, "bad_request", Some(e.to_string())),
    };
    let (tenant, sub) = (tenant.trim().to_string(), sub.trim().to_string());
    if tenant.is_empty() || sub.is_empty() {
        return fail(StatusCode::BAD_REQUEST, "bad_account", None);
    }
    let user = match st.store.find_account(&tenant, &sub, kind).await {
        Ok(Some(u)) => u,
        Ok(None) => return fail(StatusCode::NOT_FOUND, "account_not_opened", None),
        Err(e) => {
            eprintln!(
                "[money] счёт {tenant}/{sub}/{} не разрешён: {e}",
                kind.tag()
            );
            return fail(
                StatusCode::SERVICE_UNAVAILABLE,
                "identity_store_unavailable",
                None,
            );
        }
    };

    let (previous, changed) = match apply_fees(&st, user, fees, operator).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let seq = st
        .journals
        .lock()
        .await
        .get(&user)
        .and_then(|j| j.events().last())
        .map(|e| e.seq())
        .unwrap_or(0);
    if changed {
        println!(
            "[money] тариф счёта {} ({tenant}/{sub}/{}) наценка {}→{} bps, комиссия {}→{} bps оператором {}: {}",
            user.0,
            kind.tag(),
            previous.markup_bps,
            fees.markup_bps,
            previous.commission_bps,
            fees.commission_bps,
            req.operator.trim(),
            req.reason.trim()
        );
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "tenant": tenant, "sub": sub, "account_kind": kind.tag(),
            "account_id": bus::account_uuid(&tenant, &sub, kind),
            "markup_bps": fees.markup_bps, "commission_bps": fees.commission_bps,
            "previous_markup_bps": previous.markup_bps,
            "previous_commission_bps": previous.commission_bps,
            "changed": changed, "seq": seq,
        })),
    )
        .into_response()
}

async fn accounts(
    State(st): State<AppState>,
    Path((tenant, sub)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = authorize(&st, &headers) {
        return *r;
    }
    let m = marks(&st).await;
    let mut out = Vec::new();
    for kind in [AccountKind::Demo, AccountKind::Real] {
        let user = match st.store.find_account(&tenant, &sub, kind).await {
            Ok(Some(u)) => u,
            Ok(None) => continue,
            Err(e) => {
                eprintln!("[money] счета {tenant}/{sub} не прочитаны: {e}");
                return fail(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "identity_store_unavailable",
                    None,
                );
            }
        };
        let frozen = trading_frozen(&st, user).await;
        let b = st.broker.lock().await;
        let opened = b.has_account(user);
        out.push(serde_json::json!({
            "account_kind": kind.tag(),
            "account_id": bus::account_uuid(&tenant, &sub, kind),
            "opened": opened,
            "currency": CURRENCY,
            "balance_minor": b.balance(user),
            "equity_minor": b.equity(user, &m),
            "free_margin_minor": b.free_margin(user, &m),
            "used_margin_minor": opened.then(|| b.used_margin(user)),
            "open_positions": b.positions(user).len(),
            "trading_frozen": frozen,
            "leverage": b.account_leverage(user),
            "leverage_assigned": b.assigned_leverage(user).is_some(),
            "leverage_classes": super::classes_dto(&b.account_class_leverage(user)),
            "markup_bps": b.account_fees(user).markup_bps,
            "commission_bps": b.account_fees(user).commission_bps,
        }));
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({ "tenant": tenant, "sub": sub, "accounts": out })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::TOKEN_HEADER;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::sync::Mutex as StdMutex;
    use tower::ServiceExt;

    const TOKEN: &str = "test-internal-money-token-0123456789abcdef";
    const TENANT: &str = "apex-ru";
    const SUB: &str = "user-42";

    struct Linked {
        inner: persistence::MemoryStore,
        ids: StdMutex<Vec<(String, String, AccountKind)>>,
    }

    #[async_trait::async_trait]
    impl Persistence for Linked {
        async fn init(&self) -> Result<(), persistence::StoreError> {
            self.inner.init().await
        }
        async fn load_site_min_stops(&self) -> Result<Vec<(String, i64)>, persistence::StoreError> {
            self.inner.load_site_min_stops().await
        }
        async fn save_site_min_stop(
            &self,
            tenant: &str,
            bps: i64,
        ) -> Result<(), persistence::StoreError> {
            self.inner.save_site_min_stop(tenant, bps).await
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
        async fn outbox_backlog(&self) -> Result<i64, persistence::StoreError> {
            self.inner.outbox_backlog().await
        }
        async fn identity_of(
            &self,
            u: UserId,
        ) -> Result<Option<(String, String)>, persistence::StoreError> {
            let ids = self.ids.lock().unwrap();
            Ok(ids
                .get((u.0 as usize).wrapping_sub(1))
                .map(|(t, s, _)| (t.clone(), s.clone())))
        }
        async fn resolve_identity(
            &self,
            tenant: &str,
            sub: &str,
        ) -> Result<UserId, persistence::StoreError> {
            self.resolve_account(tenant, sub, AccountKind::Demo).await
        }
        async fn resolve_account(
            &self,
            tenant: &str,
            sub: &str,
            kind: AccountKind,
        ) -> Result<UserId, persistence::StoreError> {
            let mut ids = self.ids.lock().unwrap();
            let key = (tenant.to_string(), sub.to_string(), kind);
            if let Some(i) = ids.iter().position(|k| *k == key) {
                return Ok(UserId(i as u64 + 1));
            }
            ids.push(key);
            Ok(UserId(ids.len() as u64))
        }
        async fn find_account(
            &self,
            tenant: &str,
            sub: &str,
            kind: AccountKind,
        ) -> Result<Option<UserId>, persistence::StoreError> {
            let ids = self.ids.lock().unwrap();
            let key = (tenant.to_string(), sub.to_string(), kind);
            Ok(ids
                .iter()
                .position(|k| *k == key)
                .map(|i| UserId(i as u64 + 1)))
        }
        async fn account_kind_of(&self, u: UserId) -> Result<AccountKind, persistence::StoreError> {
            let ids = self.ids.lock().unwrap();
            Ok(ids
                .get((u.0 as usize).wrapping_sub(1))
                .map(|k| k.2)
                .unwrap_or_default())
        }
    }

    fn linked() -> Arc<Linked> {
        Arc::new(Linked {
            inner: persistence::MemoryStore::new(),
            ids: StdMutex::new(Vec::new()),
        })
    }

    fn state_with(store: Arc<dyn Persistence>, token: Option<&str>) -> AppState {
        let mut st = build_state_with(store);
        st.internal_money_token = token.map(Arc::from);
        st.real_trading_enabled = true;
        st.bus_real_events = true;
        st
    }

    fn state(token: Option<&str>) -> AppState {
        state_with(linked(), token)
    }

    async fn price(st: &AppState, instrument: u32, px: i64) {
        st.tickers.lock().await.insert(
            instrument,
            feed::Ticker {
                last: px,
                change: 0.0,
                high: px,
                bid: px,
                ask: px,
                ..Default::default()
            },
        );
    }

    async fn call(
        app: &Router,
        uri: &str,
        token: Option<&str>,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let mut req = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(t) = token {
            req = req.header(TOKEN_HEADER, t);
        }
        let resp = app
            .clone()
            .oneshot(
                req.body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    fn uri(op: &str) -> String {
        format!("/internal/sites/{TENANT}/users/{SUB}/real/{op}")
    }

    fn body(amount: i64, r: &str) -> serde_json::Value {
        serde_json::json!({
            "amount_minor": amount,
            "currency": "USD",
            "external_ref": r,
            "initiator": "site:crypto-watcher",
            "reason": "testnet USDT deposit",
        })
    }

    async fn queued(st: &AppState) -> Vec<serde_json::Value> {
        st.store
            .claim_outbox(100, 0)
            .await
            .unwrap()
            .iter()
            .map(|c| serde_json::from_str(&c.payload).unwrap())
            .collect()
    }

    fn balance_changes(all: &[serde_json::Value]) -> Vec<&serde_json::Value> {
        all.iter()
            .filter(|e| e["event"] == bus::EV_BALANCE_CHANGED)
            .collect()
    }

    #[tokio::test]
    async fn without_the_env_secret_the_routes_do_not_exist() {
        let app = router(state(None));
        for op in ["deposit", "withdraw"] {
            let (s, _) = call(&app, &uri(op), Some(TOKEN), body(100, "tx-1")).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{op}");
            let (s, _) = call(&app, &uri(op), None, body(100, "tx-1")).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{op}");
        }
    }

    #[tokio::test]
    async fn the_token_is_required_and_must_match() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());
        let (s, v) = call(&app, &uri("deposit"), None, body(100, "tx-1")).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{v}");
        assert_eq!(v["error"], "missing_internal_token");

        let (s, v) = call(&app, &uri("deposit"), Some("wrong"), body(100, "tx-1")).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
        assert_eq!(v["error"], "invalid_internal_token");

        let prefix = &TOKEN[..TOKEN.len() - 1];
        let (s, _) = call(&app, &uri("deposit"), Some(prefix), body(100, "tx-1")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);

        assert!(!st.broker.lock().await.has_account(DEFAULT_USER));

        let (s, v) = call(&app, &uri("deposit"), Some(TOKEN), body(100, "tx-1")).await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }

    #[test]
    fn constant_time_compare_is_still_a_compare() {
        assert!(super::ct_eq(b"abc", b"abc"));
        assert!(!super::ct_eq(b"abc", b"abd"));
        assert!(!super::ct_eq(b"abc", b"abcd"));
        assert!(!super::ct_eq(b"", b"a"));
        assert!(super::ct_eq(b"", b""));
    }

    #[test]
    fn a_blank_secret_is_no_secret() {
        assert!(super::token_from_env(None).is_none());
        assert!(super::token_from_env(Some("   ")).is_none());
        assert_eq!(
            super::token_from_env(Some(" s3cret ")).as_deref(),
            Some("s3cret")
        );
    }

    #[tokio::test]
    async fn a_deposit_credits_once_per_external_ref() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());

        let (s, first) = call(&app, &uri("deposit"), Some(TOKEN), body(25_000, "0xabc")).await;
        assert_eq!(s, StatusCode::OK, "{first}");
        let start = 0;
        assert_eq!(first["balance_minor"], serde_json::json!(start + 25_000));
        assert_eq!(first["replayed"], false);
        let seq = first["seq"].as_u64().expect("seq");
        assert!(seq >= 2, "до пополнения в журнале заведение счёта: {first}");

        let (s, again) = call(&app, &uri("deposit"), Some(TOKEN), body(25_000, "0xabc")).await;
        assert_eq!(s, StatusCode::OK, "{again}");
        assert_eq!(again["replayed"], true);
        assert_eq!(again["seq"], first["seq"]);
        assert_eq!(again["balance_minor"], first["balance_minor"]);
        assert_eq!(
            st.broker.lock().await.balance(DEFAULT_USER),
            Some(start + 25_000),
            "повтор не зачислил второй раз"
        );

        let (s, v) = call(&app, &uri("deposit"), Some(TOKEN), body(99, "0xabc")).await;
        assert_eq!(s, StatusCode::CONFLICT, "{v}");
        assert_eq!(v["error"], "external_ref_conflict");
        let (s, _) = call(&app, &uri("withdraw"), Some(TOKEN), body(25_000, "0xabc")).await;
        assert_eq!(s, StatusCode::CONFLICT);
        let (s, _) = call(
            &app,
            &format!("/internal/sites/{TENANT}/users/someone-else/real/deposit"),
            Some(TOKEN),
            body(25_000, "0xabc"),
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT);

        let (s, v) = call(&app, &uri("deposit"), Some(TOKEN), body(1, "0xdef")).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["balance_minor"], serde_json::json!(start + 25_001));
    }

    #[tokio::test]
    async fn idempotency_survives_a_restart() {
        let store = linked();
        let st = state_with(store.clone(), Some(TOKEN));
        restore_state(&st).await.unwrap();
        let app = router(st.clone());
        let (s, first) = call(&app, &uri("deposit"), Some(TOKEN), body(500, "tx-r")).await;
        assert_eq!(s, StatusCode::OK, "{first}");

        let st2 = state_with(store, Some(TOKEN));
        restore_state(&st2).await.unwrap();
        let app2 = router(st2.clone());
        let (s, again) = call(&app2, &uri("deposit"), Some(TOKEN), body(500, "tx-r")).await;
        assert_eq!(s, StatusCode::OK, "{again}");
        assert_eq!(again["replayed"], true, "{again}");
        assert_eq!(again["seq"], first["seq"]);
        assert_eq!(again["balance_minor"], first["balance_minor"]);
    }

    #[tokio::test]
    async fn the_journal_names_who_why_and_the_external_ref() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());
        let (s, v) = call(&app, &uri("deposit"), Some(TOKEN), body(700, "0xfeed")).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let js = st.journals.lock().await;
        let e = js[&DEFAULT_USER]
            .events()
            .iter()
            .find(|e| e.seq() == v["seq"].as_u64().unwrap())
            .unwrap();
        let op = e
            .origin()
            .operator()
            .expect("проводка ручная, с оператором");
        assert_eq!(op.who(), "site:crypto-watcher");
        assert_eq!(op.reason(), "testnet USDT deposit");
        assert_eq!(op.incident(), "ext:0xfeed");
        assert!(matches!(
            e.kind(),
            journal::EventKind::Deposited { amount: 700 }
        ));
    }

    #[tokio::test]
    async fn bad_bodies_are_refused_by_name() {
        let app = router(state(Some(TOKEN)));
        let cases = [
            (body(0, "r"), "invalid_amount"),
            (body(-5, "r"), "invalid_amount"),
            (
                serde_json::json!({"amount_minor":1,"currency":"EUR","external_ref":"r","initiator":"x","reason":"y"}),
                "unsupported_currency",
            ),
            (body(1, "  "), "invalid_external_ref"),
            (
                serde_json::json!({"amount_minor":1,"currency":"USD","external_ref":"r","initiator":" ","reason":"y"}),
                "missing_initiator",
            ),
            (
                serde_json::json!({"amount_minor":1,"currency":"USD","external_ref":"r","initiator":"x","reason":""}),
                "missing_reason",
            ),
            (serde_json::json!({"amount_minor":1}), "bad_request"),
        ];
        for (b, want) in cases {
            let (s, v) = call(&app, &uri("deposit"), Some(TOKEN), b.clone()).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{b} → {v}");
            assert_eq!(v["error"], want, "{b} → {v}");
        }
        let (s, v) = call(
            &app,
            &uri("deposit"),
            Some(TOKEN),
            serde_json::json!({"amount_minor":1,"currency":"usd","external_ref":"r-op","operator":"ops@site","reason":"y"}),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }

    #[tokio::test]
    async fn a_withdrawal_from_an_account_that_does_not_exist_is_refused() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());
        let (s, v) = call(&app, &uri("withdraw"), Some(TOKEN), body(1, "wd-1")).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
        assert_eq!(v["error"], "account_not_opened");
        assert!(
            !st.broker.lock().await.has_account(DEFAULT_USER),
            "отказ не завёл счёт"
        );
    }

    #[tokio::test]
    async fn a_withdrawal_beyond_free_margin_is_refused_with_the_numbers() {
        let st = state(Some(TOKEN));
        price(&st, 1, 5_000_000).await;
        let app = router(st.clone());

        let (s, _) = call(&app, &uri("deposit"), Some(TOKEN), body(10_000_000, "seed")).await;
        assert_eq!(s, StatusCode::OK);
        let balance = st.broker.lock().await.balance(DEFAULT_USER).unwrap();

        let qty = (balance / 2) * 100_000 / 5_000_000;
        let open = Request::builder()
            .method("POST")
            .uri("/deals")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({"instrument":1,"side":"buy","qty":qty}))
                    .unwrap(),
            ))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(open).await.unwrap().status(),
            StatusCode::OK
        );
        let free = {
            let m = marks(&st).await;
            st.broker
                .lock()
                .await
                .free_margin(DEFAULT_USER, &m)
                .unwrap()
        };
        assert!(free < balance);

        let too_much = i64::try_from(free + 1).unwrap();
        let (s, v) = call(
            &app,
            &uri("withdraw"),
            Some(TOKEN),
            body(too_much, "wd-big"),
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT, "{v}");
        assert_eq!(v["error"], "insufficient_free_margin");
        assert_eq!(v["free_margin_minor"], serde_json::json!(free));
        assert_eq!(
            st.broker.lock().await.balance(DEFAULT_USER),
            Some(balance),
            "отказ не тронул остаток"
        );

        let ok_amount = i64::try_from(free).unwrap();
        let (s, v) = call(
            &app,
            &uri("withdraw"),
            Some(TOKEN),
            body(ok_amount, "wd-big"),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["balance_minor"], serde_json::json!(balance - free));
        assert_eq!(v["replayed"], false);
    }

    #[tokio::test]
    async fn deposit_and_withdrawal_reach_the_bus() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());

        let (s, dep) = call(&app, &uri("deposit"), Some(TOKEN), body(10_000, "tx-bus")).await;
        assert_eq!(s, StatusCode::OK, "{dep}");
        let all = queued(&st).await;
        let changes = balance_changes(&all);
        let last = changes.last().expect("пополнение дало balance.changed");
        assert_eq!(last["data"]["reason"], "deposit", "{last}");
        assert_eq!(last["data"]["tenant"], TENANT);
        assert_eq!(last["data"]["userId"], SUB);
        assert_eq!(last["data"]["accountSeq"], dep["seq"]);
        assert_eq!(last["data"]["balanceCents"], dep["balance_minor"]);

        let (s, wd) = call(&app, &uri("withdraw"), Some(TOKEN), body(4_000, "wd-bus")).await;
        assert_eq!(s, StatusCode::OK, "{wd}");
        let all = queued(&st).await;
        let changes = balance_changes(&all);
        let last = changes.last().expect("вывод дал balance.changed");
        assert_eq!(last["data"]["accountSeq"], wd["seq"]);
        assert_eq!(last["data"]["reason"], "withdrawal", "{last}");
        assert_eq!(last["data"]["balanceCents"], wd["balance_minor"]);

        let before = balance_changes(&queued(&st).await).len();
        let (s, _) = call(&app, &uri("deposit"), Some(TOKEN), body(10_000, "tx-bus")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(balance_changes(&queued(&st).await).len(), before);
    }

    #[tokio::test]
    async fn leverage_20_margin_call_then_stop_out() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());
        const PX: i64 = 5_000_000;
        price(&st, 1, PX).await;

        let (s, v) = call(
            &app,
            &uri("deposit"),
            Some(TOKEN),
            body(1_000_000, "tx-lev"),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let balance = st.broker.lock().await.balance(DEFAULT_USER).unwrap();

        let qty = i64::try_from(balance * 19 * 100_000 / i128::from(PX)).unwrap();
        let deal = |q: i64| {
            Request::builder()
                .method("POST")
                .uri("/deals")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({"instrument":1,"side":"buy","qty":q}))
                        .unwrap(),
                ))
                .unwrap()
        };

        assert_eq!(
            app.clone().oneshot(deal(qty)).await.unwrap().status(),
            StatusCode::PAYMENT_REQUIRED
        );

        {
            let mut b = st.broker.lock().await;
            let snap = b.snapshot(DEFAULT_USER);
            let mut fresh = Broker::new(b.start_balance(), 20)
                .with_stop_out_pct(Some(50))
                .with_margin_call_pct(Some(100))
                .with_margin_call_rearm_band_pct(10)
                .with_negative_balance_protection(true);
            fresh.restore(DEFAULT_USER, snap);
            *b = fresh;
        }
        assert_eq!(
            app.clone().oneshot(deal(qty)).await.unwrap().status(),
            StatusCode::OK
        );
        {
            let m = marks(&st).await;
            let b = st.broker.lock().await;
            let used = b.used_margin(DEFAULT_USER);
            assert_eq!(used, i128::from(PX) * i128::from(qty) / 100_000 / 20);
            assert!(
                used * 100 >= balance * 94,
                "маржа ≈ 95 % остатка: {used} из {balance}"
            );
            assert!(b.margin_level_pct(DEFAULT_USER, &m).unwrap() > 100);
        }

        price(&st, 1, PX / 100 * 99).await;
        monitor_tick(&st).await;
        {
            let m = marks(&st).await;
            let b = st.broker.lock().await;
            let lvl = b.margin_level_pct(DEFAULT_USER, &m).unwrap();
            assert!((80..=90).contains(&lvl), "уровень {lvl}");
            assert_eq!(
                b.positions(DEFAULT_USER).len(),
                1,
                "до стоп-аута позиция жива"
            );
        }
        let all = queued(&st).await;
        let calls: Vec<_> = all
            .iter()
            .filter(|e| e["event"] == bus::EV_MARGIN_CALLED)
            .collect();
        assert_eq!(calls.len(), 1, "одно предупреждение: {all:?}");

        price(&st, 1, PX / 100 * 97).await;
        monitor_tick(&st).await;
        let b = st.broker.lock().await;
        assert!(
            b.positions(DEFAULT_USER).is_empty(),
            "стоп-аут закрыл позицию"
        );
        let after = b.balance(DEFAULT_USER).unwrap();
        let loss = i128::from(PX / 100 * 3) * i128::from(qty) / 100_000;
        assert_eq!(after, balance - loss, "убыток — ровно 3 % нотионала");
        assert!(after > 0, "до минуса не дошло: {after}");
        drop(b);
        let all = queued(&st).await;
        assert!(
            all.iter().any(|e| e["event"] == bus::EV_TRADE_EXECUTED),
            "закрытие ушло на шину: {all:?}"
        );
        assert_eq!(
            balance_changes(&all).last().unwrap()["data"]["reason"],
            "trade"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn parallel_repeats_of_one_ref_credit_once() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());
        let u = uri("deposit");
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let app = app.clone();
                let u = u.clone();
                tokio::spawn(
                    async move { call(&app, &u, Some(TOKEN), body(123_456, "0xrace")).await },
                )
            })
            .collect();
        let mut fresh = 0;
        for t in tasks {
            let (s, v) = t.await.unwrap();
            assert_eq!(s, StatusCode::OK, "{v}");
            if v["replayed"] == false {
                fresh += 1;
            }
        }
        assert_eq!(fresh, 1, "проводка ровно одна");
        let start = 0;
        assert_eq!(
            st.broker.lock().await.balance(DEFAULT_USER),
            Some(start + 123_456)
        );
    }

    fn freeze_body(frozen: bool) -> serde_json::Value {
        serde_json::json!({
            "frozen": frozen,
            "reason": "AML: проверка источника средств",
            "operator": "aml@site",
            "external_ref": "case-7"
        })
    }

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        b: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let s = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            s,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    fn deal(sl: Option<i64>) -> serde_json::Value {
        serde_json::json!({"instrument": 1, "side": "buy", "qty": 1000, "sl": sl})
    }

    #[tokio::test]
    async fn the_freeze_route_is_behind_the_same_secret() {
        let app = router(state(None));
        let (s, _) = call(&app, &uri("trading-freeze"), Some(TOKEN), freeze_body(true)).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let app = router(state(Some(TOKEN)));
        let (s, _) = call(&app, &uri("trading-freeze"), None, freeze_body(true)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, _) = call(
            &app,
            &uri("trading-freeze"),
            Some("nope"),
            freeze_body(true),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, v) = call(
            &app,
            &uri("trading-freeze"),
            Some(TOKEN),
            serde_json::json!({"frozen": true, "reason": "", "operator": "x"}),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    }

    #[tokio::test]
    async fn a_frozen_account_can_close_and_deposit_but_not_open_or_withdraw() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());
        price(&st, 1, 5_000_000).await;
        let (s, _) = call(
            &app,
            &uri("deposit"),
            Some(TOKEN),
            body(10_000_000, "seed-f"),
        )
        .await;
        assert_eq!(s, StatusCode::OK);

        let (s, v) = send(&app, "POST", "/deals", deal(None)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let (s, v) = send(&app, "POST", "/deals", deal(Some(4_900_000))).await;
        assert_eq!(s, StatusCode::OK, "{v}");

        let (s, v) = call(&app, &uri("trading-freeze"), Some(TOKEN), freeze_body(true)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["changed"], true);
        assert_eq!(v["frozen"], true);
        let frozen_seq = v["seq"].clone();

        let (s, v) = call(&app, &uri("trading-freeze"), Some(TOKEN), freeze_body(true)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["changed"], false);
        assert_eq!(v["seq"], frozen_seq);

        let (s, v) = send(&app, "POST", "/deals", deal(None)).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
        assert_eq!(v["error"], "account_frozen");
        let pending = serde_json::json!({"instrument":1,"side":"buy","qty":1000,"price":4_000_000});
        let (s, v) = send(&app, "POST", "/pending", pending).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
        assert_eq!(v["error"], "account_frozen");
        let (s, v) = call(&app, &uri("withdraw"), Some(TOKEN), body(1, "wd-frozen")).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
        assert_eq!(v["error"], "account_frozen");
        let (s, v) = send(&app, "POST", "/account/demo-reset", serde_json::json!({})).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{v}");

        let (s, v) = call(&app, &uri("deposit"), Some(TOKEN), body(500, "dep-frozen")).await;
        assert_eq!(s, StatusCode::OK, "пополнение замороженному разрешено: {v}");
        let (s, v) = send(&app, "POST", "/deals/1/close", serde_json::json!({})).await;
        assert_eq!(s, StatusCode::OK, "закрытие разрешено: {v}");

        price(&st, 1, 4_800_000).await;
        monitor_tick(&st).await;
        assert!(st.broker.lock().await.positions(DEFAULT_USER).is_empty());

        let (_, acc) = send(&app, "GET", "/account", serde_json::Value::Null).await;
        assert_eq!(acc["trading_frozen"], true, "{acc}");

        {
            let js = st.journals.lock().await;
            let e = js[&DEFAULT_USER]
                .events()
                .iter()
                .find(|e| {
                    matches!(
                        e.kind(),
                        journal::EventKind::TradingFreezeSet { frozen: true }
                    )
                })
                .expect("запись о заморозке");
            let op = e.origin().operator().unwrap();
            assert_eq!(op.who(), "aml@site");
            assert_eq!(op.incident(), "ext:case-7");
        }
        assert!(st.quarantined.lock().await.is_empty(), "сверка сошлась");

        let (s, v) = call(
            &app,
            &uri("trading-freeze"),
            Some(TOKEN),
            freeze_body(false),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["changed"], true);
        price(&st, 1, 5_000_000).await;
        let (s, v) = send(&app, "POST", "/deals", deal(None)).await;
        assert_eq!(
            s,
            StatusCode::OK,
            "после разморозки торговля вернулась: {v}"
        );
    }

    #[tokio::test]
    async fn a_freeze_survives_a_restart() {
        let store = linked();
        let st = state_with(store.clone(), Some(TOKEN));
        restore_state(&st).await.unwrap();
        let app = router(st.clone());
        let (s, v) = call(&app, &uri("trading-freeze"), Some(TOKEN), freeze_body(true)).await;
        assert_eq!(s, StatusCode::OK, "{v}");

        let st2 = state_with(store, Some(TOKEN));
        restore_state(&st2).await.unwrap();
        assert!(
            st2.quarantined.lock().await.is_empty(),
            "лента поднялась чисто"
        );
        price(&st2, 1, 5_000_000).await;
        let app2 = router(st2.clone());
        let (s, v) = send(&app2, "POST", "/deals", deal(None)).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
        assert_eq!(v["error"], "account_frozen");
    }

    async fn get(app: &Router, uri: &str, token: Option<&str>) -> (StatusCode, serde_json::Value) {
        let mut req = Request::builder().method("GET").uri(uri);
        if let Some(t) = token {
            req = req.header(TOKEN_HEADER, t);
        }
        let resp = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let s = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            s,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    fn accounts_uri() -> String {
        format!("/internal/sites/{TENANT}/users/{SUB}/accounts")
    }

    #[tokio::test]
    async fn money_lands_on_the_real_account_only() {
        let store = linked();
        let st = state_with(store.clone(), Some(TOKEN));
        let app = router(st.clone());

        let (s, v) = get(&app, &accounts_uri(), Some(TOKEN)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["accounts"], serde_json::json!([]));

        let (s, dep) = call(&app, &uri("deposit"), Some(TOKEN), body(2_500, "tx-real")).await;
        assert_eq!(s, StatusCode::OK, "{dep}");
        assert_eq!(dep["account_kind"], "real");
        assert_eq!(
            dep["balance_minor"], 2_500,
            "реальный счёт открыт нулём: {dep}"
        );
        let real = store
            .find_account(TENANT, SUB, AccountKind::Real)
            .await
            .unwrap()
            .expect("реальный счёт заведён пополнением");
        assert_eq!(
            st.broker.lock().await.snapshot(real).profile.kind,
            AccountKind::Real
        );

        let demo = store
            .resolve_account(TENANT, SUB, AccountKind::Demo)
            .await
            .unwrap();
        assert_ne!(demo, real);
        let (_, v) = get(&app, &accounts_uri(), Some(TOKEN)).await;
        let list = v["accounts"].as_array().unwrap();
        assert_eq!(list.len(), 2, "{v}");
        let by = |k: &str| {
            list.iter()
                .find(|a| a["account_kind"] == k)
                .unwrap()
                .clone()
        };
        assert_eq!(by("real")["balance_minor"], 2_500);
        assert_eq!(by("real")["account_id"], dep["account_id"]);
        assert_eq!(by("demo")["opened"], false);
        assert!(by("demo")["balance_minor"].is_null(), "{v}");
        assert_ne!(by("demo")["account_id"], by("real")["account_id"]);

        for op in ["deposit", "withdraw"] {
            let (s, _) = call(
                &app,
                &format!("/internal/sites/{TENANT}/users/{SUB}/demo/{op}"),
                Some(TOKEN),
                body(1, "tx-demo"),
            )
            .await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{op} на демо");
        }
        let (s, _) = get(&app, &accounts_uri(), None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bus_events_name_the_account() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());
        let (s, dep) = call(&app, &uri("deposit"), Some(TOKEN), body(700, "tx-kind")).await;
        assert_eq!(s, StatusCode::OK, "{dep}");
        let all = queued(&st).await;
        let opened = all
            .iter()
            .find(|e| e["event"] == bus::EV_ACCOUNT_OPENED)
            .expect("account.opened");
        assert_eq!(opened["data"]["accountKind"], "real");
        assert_eq!(opened["data"]["accountId"], dep["account_id"]);
        assert_eq!(opened["data"]["balanceCents"], 0);
        assert_ne!(
            opened["event_id"],
            serde_json::Value::from(bus::event_id(
                bus::EV_ACCOUNT_OPENED,
                &format!("{TENANT}:{SUB}")
            )),
            "ключ демо-счёта занят демо-счётом"
        );
        let changed = balance_changes(&all);
        assert_eq!(changed.last().unwrap()["data"]["accountKind"], "real");
    }

    #[tokio::test]
    async fn real_account_events_stay_home_until_enabled() {
        let mut st = state(Some(TOKEN));
        st.bus_real_events = false;
        let app = router(st.clone());
        let (s, _) = call(&app, &uri("deposit"), Some(TOKEN), body(700, "tx-quiet")).await;
        assert_eq!(s, StatusCode::OK);
        assert!(queued(&st).await.is_empty(), "реальный счёт молчит на шине");
    }

    #[tokio::test]
    async fn real_trading_is_closed_unless_enabled() {
        let mut st = state(Some(TOKEN));
        st.real_trading_enabled = false;
        price(&st, 1, 5_000_000).await;
        let app = router(st.clone());
        let (s, _) = call(
            &app,
            &uri("deposit"),
            Some(TOKEN),
            body(10_000_000, "tx-gate"),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let (s, v) = send(&app, "POST", "/deals", deal(None)).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
        assert_eq!(v["error"], "identity_not_verified");
    }

    #[tokio::test]
    async fn the_account_names_its_kind() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());
        let (_, dep) = call(&app, &uri("deposit"), Some(TOKEN), body(1, "tx-badge")).await;
        let (_, acc) = send(&app, "GET", "/account", serde_json::Value::Null).await;
        assert_eq!(acc["kind"], "real", "{acc}");
        assert_eq!(acc["account_id"], dep["account_id"]);
    }

    #[tokio::test]
    async fn the_demo_account_can_be_frozen_too() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());
        let (s, v) = call(
            &app,
            &format!("/internal/sites/{TENANT}/users/{SUB}/demo/trading-freeze"),
            Some(TOKEN),
            freeze_body(true),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["account_kind"], "demo");
        let (s, _) = call(
            &app,
            &format!("/internal/sites/{TENANT}/users/{SUB}/gold/trading-freeze"),
            Some(TOKEN),
            freeze_body(true),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }

    fn lev_uri(kind: &str) -> String {
        format!("/internal/sites/{TENANT}/users/{SUB}/{kind}/leverage")
    }

    fn lev_body(leverage: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "leverage": leverage,
            "reason": "смена портфеля: Standard → Pro",
            "operator": "risk@site",
            "external_ref": "adm-journal-42"
        })
    }

    async fn position_levs(st: &AppState) -> Vec<i128> {
        let mut v: Vec<(u64, i128)> = st
            .broker
            .lock()
            .await
            .positions(DEFAULT_USER)
            .iter()
            .map(|p| (p.id, p.leverage))
            .collect();
        v.sort_unstable();
        v.into_iter().map(|(_, l)| l).collect()
    }

    #[tokio::test]
    async fn the_leverage_route_is_behind_the_secret_and_validates() {
        let app = router(state(None));
        let (s, _) = call(&app, &lev_uri("real"), Some(TOKEN), lev_body(50.into())).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "без секрета маршрута нет");

        let st = state(Some(TOKEN));
        let app = router(st.clone());
        let (s, v) = call(&app, &lev_uri("real"), None, lev_body(50.into())).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{v}");
        let (s, _) = call(&app, &lev_uri("real"), Some("nope"), lev_body(50.into())).await;
        assert_eq!(s, StatusCode::FORBIDDEN);

        let (s, _) = call(&app, &uri("deposit"), Some(TOKEN), body(1, "seed-l")).await;
        assert_eq!(s, StatusCode::OK);

        for bad in [0, -5, 1001] {
            let (s, v) = call(&app, &lev_uri("real"), Some(TOKEN), lev_body(bad.into())).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {v}");
            assert_eq!(v["error"], "invalid_leverage", "{bad}");
        }
        let (s, v) = call(&app, &lev_uri("real"), Some(TOKEN), lev_body("20".into())).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "строка — не число: {v}");
        let (s, v) = call(&app, &lev_uri("real"), Some(TOKEN), lev_body(2.5.into())).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "дробь — не плечо: {v}");
        let (s, v) = call(
            &app,
            &lev_uri("real"),
            Some(TOKEN),
            serde_json::json!({"leverage": 20, "reason": " ", "operator": "x"}),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"], "missing_reason");
        let (s, v) = call(
            &app,
            &lev_uri("real"),
            Some(TOKEN),
            serde_json::json!({"leverage": 20, "reason": "r", "operator": ""}),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"], "missing_operator");
        let (s, _) = call(&app, &lev_uri("gold"), Some(TOKEN), lev_body(20.into())).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "неизвестный род счёта");

        assert_eq!(st.broker.lock().await.assigned_leverage(DEFAULT_USER), None);

        let (s, v) = call(&app, &lev_uri("demo"), Some(TOKEN), lev_body(20.into())).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
        assert_eq!(v["error"], "account_not_opened");
    }

    #[tokio::test]
    async fn leverage_reaches_new_positions_only_and_survives_a_restart() {
        let store = linked();
        let st = state_with(store.clone(), Some(TOKEN));
        restore_state(&st).await.unwrap();
        let app = router(st.clone());
        price(&st, 1, 5_000_000).await;
        let (s, _) = call(
            &app,
            &uri("deposit"),
            Some(TOKEN),
            body(100_000_000, "seed-lv"),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let platform = st.broker.lock().await.leverage();

        let (s, v) = send(&app, "POST", "/deals", deal(None)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let (_, acc) = send(&app, "GET", "/account", serde_json::Value::Null).await;
        assert_eq!(
            acc["leverage"],
            serde_json::json!(platform),
            "не назначено — плечо площадки"
        );

        let (s, v) = call(&app, &lev_uri("real"), Some(TOKEN), lev_body(50.into())).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["changed"], true);
        assert_eq!(v["leverage"], 50);
        assert_eq!(v["previous_leverage"], serde_json::json!(platform));
        let seq = v["seq"].clone();
        let (s, v) = call(&app, &lev_uri("real"), Some(TOKEN), lev_body(50.into())).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["changed"], false, "повтор — без записи");
        assert_eq!(v["seq"], seq);

        let (s, v) = send(&app, "POST", "/deals", deal(None)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(
            position_levs(&st).await,
            vec![platform, 50],
            "старая позиция — со своим плечом, новая — с плечом счёта"
        );
        let (_, acc) = send(&app, "GET", "/account", serde_json::Value::Null).await;
        assert_eq!(acc["leverage"], 50, "{acc}");

        {
            let js = st.journals.lock().await;
            let e = js[&DEFAULT_USER]
                .events()
                .iter()
                .find(|e| matches!(e.kind(), journal::EventKind::LeverageSet { lev: 50 }))
                .expect("запись о смене плеча");
            let op = e.origin().operator().unwrap();
            assert_eq!(op.who(), "risk@site");
            assert_eq!(op.incident(), "ext:adm-journal-42");
        }

        let st2 = state_with(store, Some(TOKEN));
        restore_state(&st2).await.unwrap();
        assert!(st2.quarantined.lock().await.is_empty(), "сверка сошлась");
        assert_eq!(st2.broker.lock().await.account_leverage(DEFAULT_USER), 50);
        assert_eq!(position_levs(&st2).await, vec![platform, 50]);
        let (s, v) = get(&router(st2.clone()), &accounts_uri(), Some(TOKEN)).await;
        assert_eq!(s, StatusCode::OK);
        let real = v["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["account_kind"] == "real")
            .expect("реальный счёт в списке")
            .clone();
        assert_eq!(real["leverage"], 50, "{real}");
        assert_eq!(real["leverage_assigned"], true);
    }

    fn table_body(legacy: i64, base: i64, classes: serde_json::Value) -> serde_json::Value {
        let mut b = lev_body(legacy.into());
        b["leverage_base"] = base.into();
        b["leverage_classes"] = classes;
        b
    }

    async fn seeded(store: Arc<Linked>, classes_on: bool) -> (AppState, Router) {
        let mut st = state_with(store, Some(TOKEN));
        st.leverage_classes_enabled = classes_on;
        restore_state(&st).await.unwrap();
        let app = router(st.clone());
        price(&st, 1, 5_000_000).await;
        let (s, _) = call(
            &app,
            &uri("deposit"),
            Some(TOKEN),
            body(100_000_000, "seed-lc"),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        (st, app)
    }

    fn has_class_record(st_journal: &journal::Journal) -> bool {
        st_journal
            .events()
            .iter()
            .any(|e| matches!(e.kind(), journal::EventKind::LeverageClassesSet { .. }))
    }

    #[tokio::test]
    async fn the_category_table_reaches_new_positions_and_survives_a_restart() {
        let store = linked();
        let (st, app) = seeded(store.clone(), true).await;

        for (bad, code) in [
            (serde_json::json!({"fx": 10}), "invalid_leverage_class"),
            (serde_json::json!({"crypto": 0}), "invalid_leverage"),
        ] {
            let (s, v) = call(&app, &lev_uri("real"), Some(TOKEN), table_body(10, 50, bad)).await;
            assert_eq!(
                (s, v["error"].as_str()),
                (StatusCode::BAD_REQUEST, Some(code))
            );
        }

        let (s, v) = call(
            &app,
            &lev_uri("real"),
            Some(TOKEN),
            table_body(10, 50, serde_json::json!({"crypto": 5, "forex": 100})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["changed"], true);
        assert_eq!(v["leverage_base"], 50);
        assert_eq!(
            v["leverage_classes"],
            serde_json::json!({"crypto": 5, "forex": 100})
        );
        assert_eq!(v["previous_leverage_classes"], serde_json::json!({}));

        let (s, v) = send(&app, "POST", "/deals", deal(None)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let (_, acc) = send(&app, "GET", "/account", serde_json::Value::Null).await;
        assert_eq!(acc["leverage"], 50);
        assert_eq!(
            acc["leverage_classes"],
            serde_json::json!({"crypto": 5, "forex": 100})
        );
        assert!(has_class_record(&st.journals.lock().await[&DEFAULT_USER]));

        let (s, v) = call(&app, &lev_uri("real"), Some(TOKEN), lev_body(20.into())).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(
            v["leverage_classes"],
            serde_json::json!({}),
            "число без таблицы — одно плечо"
        );
        assert_eq!(
            v["previous_leverage_classes"],
            serde_json::json!({"crypto": 5, "forex": 100})
        );
        let (s, v) = send(&app, "POST", "/deals", deal(None)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(position_levs(&st).await, vec![5, 20]);

        let (s, v) = call(
            &app,
            &lev_uri("real"),
            Some(TOKEN),
            table_body(10, 50, serde_json::json!({"crypto": 7})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");

        let st2 = state_with(store, Some(TOKEN));
        restore_state(&st2).await.unwrap();
        assert!(st2.quarantined.lock().await.is_empty(), "сверка сошлась");
        assert_eq!(
            position_levs(&st2).await,
            vec![5, 20],
            "открытые позиции не пересчитаны"
        );
        let b = st2.broker.lock().await;
        assert_eq!(b.account_leverage(DEFAULT_USER), 50);
        assert_eq!(b.instrument_leverage(DEFAULT_USER, 1), 7);
    }

    #[tokio::test]
    async fn with_the_flag_off_the_table_is_ignored_and_nothing_new_is_written() {
        let (st, app) = seeded(linked(), false).await;
        let (s, v) = call(
            &app,
            &lev_uri("real"),
            Some(TOKEN),
            table_body(10, 50, serde_json::json!({"crypto": 5, "nope": 1})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["leverage"], 10);
        assert!(v.get("leverage_classes").is_none(), "{v}");
        let (s, v) = send(&app, "POST", "/deals", deal(None)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(
            position_levs(&st).await,
            vec![10],
            "как раньше — одно число"
        );
        assert!(!has_class_record(&st.journals.lock().await[&DEFAULT_USER]));
        let (_, acc) = send(&app, "GET", "/account", serde_json::Value::Null).await;
        assert_eq!(acc["leverage_classes"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn the_sso_claim_carries_the_table_only_with_the_flag_on() {
        for on in [false, true] {
            let mut st = state(Some(TOKEN));
            st.leverage_classes_enabled = on;
            let user = st
                .store
                .resolve_account(TENANT, SUB, AccountKind::Demo)
                .await
                .unwrap();
            let classes = std::collections::BTreeMap::from([("forex".to_string(), 100i64)]);
            sync_leverage_claim(&st, user, Some(10), Some(30), Some(&classes))
                .await
                .unwrap();
            let b = st.broker.lock().await;
            if on {
                assert_eq!(b.account_leverage(user), 30);
                assert_eq!(b.account_class_leverage(user).len(), 1);
            } else {
                assert_eq!(b.account_leverage(user), 10);
                assert!(b.account_class_leverage(user).is_empty());
            }
            drop(b);
            if on {
                let bad = std::collections::BTreeMap::from([("fx".to_string(), 100i64)]);
                let e = sync_leverage_claim(&st, user, Some(10), Some(30), Some(&bad))
                    .await
                    .unwrap_err();
                assert_eq!(e.0, StatusCode::BAD_REQUEST);
            }
        }
    }

    #[tokio::test]
    async fn a_pending_order_placed_before_a_freeze_is_rejected_when_it_fires() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());
        price(&st, 1, 5_000_000).await;
        let (s, _) = call(
            &app,
            &uri("deposit"),
            Some(TOKEN),
            body(100_000_000, "seed-pf"),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let pending = serde_json::json!({"instrument":1,"side":"buy","qty":1000,"price":4_900_000});
        let (s, v) = send(&app, "POST", "/pending", pending).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let (s, v) = call(&app, &uri("trading-freeze"), Some(TOKEN), freeze_body(true)).await;
        assert_eq!(s, StatusCode::OK, "{v}");

        price(&st, 1, 4_800_000).await;
        monitor_tick(&st).await;
        {
            let b = st.broker.lock().await;
            assert!(b.positions(DEFAULT_USER).is_empty(), "позиция не открыта");
            assert!(b.pendings(DEFAULT_USER).is_empty(), "ордер снят");
        }
        {
            let js = st.journals.lock().await;
            let e = js[&DEFAULT_USER].events().last().expect("запись монитора");
            assert!(
                matches!(
                    e.kind(),
                    journal::EventKind::PendingRejected {
                        reason: journal::RejectReason::AccountFrozen,
                        exec: Some(4_800_000),
                        ..
                    }
                ),
                "{}",
                e.canon()
            );
            assert_eq!(e.origin(), &journal::Origin::System);
        }
        assert!(st.quarantined.lock().await.is_empty(), "сверка сошлась");
    }

    #[tokio::test]
    async fn the_same_pending_order_fires_on_an_unfrozen_account() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());
        price(&st, 1, 5_000_000).await;
        let (s, _) = call(
            &app,
            &uri("deposit"),
            Some(TOKEN),
            body(100_000_000, "seed-pu"),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let pending = serde_json::json!({"instrument":1,"side":"buy","qty":1000,"price":4_900_000});
        let (s, _) = send(&app, "POST", "/pending", pending).await;
        assert_eq!(s, StatusCode::OK);
        price(&st, 1, 4_800_000).await;
        monitor_tick(&st).await;
        assert_eq!(st.broker.lock().await.positions(DEFAULT_USER).len(), 1);
    }

    #[tokio::test]
    async fn the_sso_leverage_claim_syncs_the_account() {
        let st = state(Some(TOKEN));
        let user = st
            .store
            .resolve_account(TENANT, SUB, AccountKind::Demo)
            .await
            .unwrap();
        sync_leverage_claim(&st, user, None, None, None)
            .await
            .unwrap();
        assert!(
            !st.broker.lock().await.has_account(user),
            "без claim-а ничего"
        );

        sync_leverage_claim(&st, user, Some(10), None, None)
            .await
            .unwrap();
        assert_eq!(st.broker.lock().await.assigned_leverage(user), Some(10));
        let n = st.journals.lock().await[&user].len();
        sync_leverage_claim(&st, user, Some(10), None, None)
            .await
            .unwrap();
        assert_eq!(st.journals.lock().await[&user].len(), n, "повтор не пишет");

        for bad in [0, 1001] {
            let e = sync_leverage_claim(&st, user, Some(bad), None, None)
                .await
                .unwrap_err();
            assert_eq!(e.0, StatusCode::BAD_REQUEST);
        }
        assert_eq!(st.broker.lock().await.assigned_leverage(user), Some(10));
        let js = st.journals.lock().await;
        let op = js[&user]
            .events()
            .iter()
            .find(|e| matches!(e.kind(), journal::EventKind::LeverageSet { .. }))
            .and_then(|e| e.origin().operator().cloned())
            .expect("запись с основанием");
        assert_eq!(op.who(), "cabinet-sso");
    }

    #[test]
    fn the_leverage_claim_is_optional() {
        let c: sso::Claims = serde_json::from_str(r#"{"sub":"u","tenant":"t","jti":"j"}"#).unwrap();
        assert_eq!(c.leverage, None);
        let c: sso::Claims =
            serde_json::from_str(r#"{"sub":"u","tenant":"t","jti":"j","leverage":20}"#).unwrap();
        assert_eq!(c.leverage, Some(20));
    }

    fn fees_uri(kind: &str) -> String {
        format!("/internal/sites/{TENANT}/users/{SUB}/{kind}/fees")
    }

    fn fees_body(markup: serde_json::Value, commission: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "markup_bps": markup,
            "commission_bps": commission,
            "reason": "портфель Pro: комиссия 10 bps",
            "operator": "risk@site",
            "external_ref": "adm-fees-7"
        })
    }

    #[tokio::test]
    async fn the_fees_route_is_behind_the_secret_and_validates() {
        let app = router(state(None));
        let (s, _) = call(
            &app,
            &fees_uri("real"),
            Some(TOKEN),
            fees_body(1.into(), 1.into()),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "без секрета маршрута нет");

        let st = state(Some(TOKEN));
        let app = router(st.clone());
        let (s, _) = call(&app, &fees_uri("real"), None, fees_body(1.into(), 1.into())).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, _) = call(
            &app,
            &fees_uri("real"),
            Some("nope"),
            fees_body(1.into(), 1.into()),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN);

        let (s, _) = call(&app, &uri("deposit"), Some(TOKEN), body(1, "seed-f")).await;
        assert_eq!(s, StatusCode::OK);

        for (m, c) in [(-1, 0), (0, -1), (501, 0), (0, 501)] {
            let (s, v) = call(
                &app,
                &fees_uri("real"),
                Some(TOKEN),
                fees_body(m.into(), c.into()),
            )
            .await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{m}/{c}: {v}");
            assert_eq!(v["error"], "invalid_fees", "{m}/{c}");
        }
        let (s, v) = call(
            &app,
            &fees_uri("real"),
            Some(TOKEN),
            fees_body("5".into(), 0.into()),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "строка — не ставка: {v}");
        let (s, v) = call(
            &app,
            &fees_uri("real"),
            Some(TOKEN),
            fees_body(0.into(), 2.5.into()),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "дробь — не bps: {v}");
        let (s, v) = call(
            &app,
            &fees_uri("real"),
            Some(TOKEN),
            serde_json::json!({"markup_bps": 1, "reason": "r", "operator": "x"}),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "обе ставки обязательны: {v}");
        let (s, v) = call(
            &app,
            &fees_uri("real"),
            Some(TOKEN),
            serde_json::json!({"markup_bps": 1, "commission_bps": 1, "reason": " ", "operator": "x"}),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"], "missing_reason");
        let (s, v) = call(
            &app,
            &fees_uri("real"),
            Some(TOKEN),
            serde_json::json!({"markup_bps": 1, "commission_bps": 1, "reason": "r", "operator": ""}),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"], "missing_operator");
        let (s, _) = call(
            &app,
            &fees_uri("gold"),
            Some(TOKEN),
            fees_body(1.into(), 1.into()),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "неизвестный род счёта");
        assert_eq!(
            st.broker.lock().await.account_fees(DEFAULT_USER),
            broker::Fees::default(),
            "ни один отказ ничего не записал"
        );
        let (s, v) = call(
            &app,
            &fees_uri("demo"),
            Some(TOKEN),
            fees_body(1.into(), 1.into()),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
        assert_eq!(v["error"], "account_not_opened");
    }

    #[tokio::test]
    async fn fees_reach_new_trades_and_the_bus_and_survive_a_restart() {
        let store = linked();
        let st = state_with(store.clone(), Some(TOKEN));
        restore_state(&st).await.unwrap();
        let app = router(st.clone());
        let px: i64 = 5_000_000;
        price(&st, 1, px).await;
        let (s, _) = call(
            &app,
            &uri("deposit"),
            Some(TOKEN),
            body(100_000_000, "seed-fe"),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let fi = feed_by_id(&st, 1).unwrap();
        let (pd, qd) = (fi.price_decimals, fi.qty_decimals);

        let (s, v) = send(&app, "POST", "/deals", deal(None)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let old = v["id"].as_u64().unwrap();
        assert_eq!(v["entry"], px, "без тарифа — рыночная цена");

        let (s, v) = call(
            &app,
            &fees_uri("real"),
            Some(TOKEN),
            fees_body(10.into(), 10.into()),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["changed"], true);
        assert_eq!(v["markup_bps"], 10);
        assert_eq!(v["commission_bps"], 10);
        assert_eq!(v["previous_markup_bps"], 0);
        let seq = v["seq"].clone();
        let (s, v) = call(
            &app,
            &fees_uri("real"),
            Some(TOKEN),
            fees_body(10.into(), 10.into()),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["changed"], false, "повтор — без записи");
        assert_eq!(v["seq"], seq);

        let (_, acc) = send(&app, "GET", "/account", serde_json::Value::Null).await;
        assert_eq!(acc["markup_bps"], 10, "{acc}");
        assert_eq!(acc["commission_bps"], 10, "{acc}");

        let balance_before = st.broker.lock().await.balance(DEFAULT_USER).unwrap();
        let (s, v) = send(&app, "POST", "/deals", deal(None)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let new = v["id"].as_u64().unwrap();
        let exec_in = broker::apply_markup(true, px, 10);
        assert_eq!(v["entry"], exec_in, "покупка дороже ask на наценку");
        let open_fee = broker::commission_for(exec_in, 1000, pd, qd, 10, 100);
        assert!(open_fee > 0);
        assert_eq!(
            st.broker.lock().await.balance(DEFAULT_USER),
            Some(balance_before - open_fee),
            "комиссия открытия списана"
        );
        {
            let js = st.journals.lock().await;
            let e = js[&DEFAULT_USER]
                .events()
                .iter()
                .find(|e| matches!(e.kind(), journal::EventKind::FeesSet { .. }))
                .expect("запись о тарифе");
            let op = e.origin().operator().unwrap();
            assert_eq!(op.who(), "risk@site");
            assert_eq!(op.incident(), "ext:adm-fees-7");
            assert_eq!(e.canon(), "fees_set markup_bps=10 commission_bps=10");
        }

        let (s, _) = send(
            &app,
            "POST",
            &format!("/deals/{old}/close"),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let (s, _) = send(
            &app,
            "POST",
            &format!("/deals/{new}/close"),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let exec_out = broker::apply_markup(false, px, 10);
        let close_fee = broker::commission_for(exec_out, 1000, pd, qd, 10, 100);

        let (_, closed) = send(&app, "GET", "/deals/closed", serde_json::Value::Null).await;
        let closed = closed.as_array().unwrap().clone();
        assert_eq!(closed[0]["exit"], exec_out, "продажа дешевле рынка");
        assert_eq!(
            closed[0]["commission"],
            (open_fee + close_fee) as i64,
            "{}",
            closed[0]
        );
        assert_eq!(closed[0]["market_exit"], px);
        assert_eq!(
            closed[1]["exit"], px,
            "старая — по своему (нулевому) тарифу"
        );
        assert_eq!(closed[1]["commission"], 0);

        let trades: Vec<serde_json::Value> = queued(&st)
            .await
            .into_iter()
            .filter(|e| e["event"] == bus::EV_TRADE_EXECUTED)
            .collect();
        assert_eq!(trades.len(), 2, "{trades:?}");
        let t_old = &trades[0]["data"];
        assert_eq!(t_old["commissionMinor"], 0);
        assert_eq!(t_old["markupMinor"], 0);
        let t = &trades[1]["data"];
        assert_eq!(t["commissionMinor"], (open_fee + close_fee) as i64, "{t}");
        let mk = broker::markup_amount(px, exec_in, 1000, pd, qd, 100)
            + broker::markup_amount(px, exec_out, 1000, pd, qd, 100);
        assert_eq!(t["markupMinor"], mk as i64, "{t}");
        assert_eq!(
            t["marketPrice"],
            serde_json::from_str::<serde_json::Value>(&bus::decimal(px, pd)).unwrap()
        );
        assert_eq!(
            t["price"],
            serde_json::from_str::<serde_json::Value>(&bus::decimal(exec_out, pd)).unwrap(),
            "price — цена исполнения"
        );
        assert_eq!(t["currency"], "USD");
        assert_eq!(t["currencyMinorUnits"], 2);

        let st2 = state_with(store, Some(TOKEN));
        restore_state(&st2).await.unwrap();
        assert!(st2.quarantined.lock().await.is_empty(), "сверка сошлась");
        assert_eq!(
            st2.broker.lock().await.account_fees(DEFAULT_USER),
            broker::Fees {
                markup_bps: 10,
                commission_bps: 10
            }
        );
        let (s, v) = get(&router(st2.clone()), &accounts_uri(), Some(TOKEN)).await;
        assert_eq!(s, StatusCode::OK);
        let real = v["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["account_kind"] == "real")
            .expect("реальный счёт в списке")
            .clone();
        assert_eq!(real["markup_bps"], 10, "{real}");
        assert_eq!(real["commission_bps"], 10, "{real}");
    }

    #[tokio::test]
    async fn an_opening_commission_is_a_trade_balance_change() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());
        price(&st, 1, 5_000_000).await;
        let (s, _) = call(
            &app,
            &uri("deposit"),
            Some(TOKEN),
            body(100_000_000, "seed-fb"),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let (s, _) = call(
            &app,
            &fees_uri("real"),
            Some(TOKEN),
            fees_body(0.into(), 10.into()),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let (s, v) = send(&app, "POST", "/deals", deal(None)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let all = queued(&st).await;
        let last = *balance_changes(&all).last().expect("событие баланса");
        assert_eq!(last["data"]["balanceMinor"], 100_000_000 - 50, "{last}");
        assert_eq!(last["data"]["reason"], "trade", "{last}");
    }

    #[tokio::test]
    async fn the_sso_fee_claims_sync_the_account() {
        let st = state(Some(TOKEN));
        let user = st
            .store
            .resolve_account(TENANT, SUB, AccountKind::Demo)
            .await
            .unwrap();
        sync_fees_claim(&st, user, None, None).await.unwrap();
        assert!(
            !st.broker.lock().await.has_account(user),
            "без claim-ов ничего"
        );

        sync_fees_claim(&st, user, Some(15), Some(0)).await.unwrap();
        let f = |m, c| broker::Fees {
            markup_bps: m,
            commission_bps: c,
        };
        assert_eq!(st.broker.lock().await.account_fees(user), f(15, 0));
        let n = st.journals.lock().await[&user].len();
        sync_fees_claim(&st, user, Some(15), Some(0)).await.unwrap();
        assert_eq!(st.journals.lock().await[&user].len(), n, "повтор не пишет");

        sync_fees_claim(&st, user, None, Some(7)).await.unwrap();
        assert_eq!(
            st.broker.lock().await.account_fees(user),
            f(15, 7),
            "один claim — вторая ставка прежняя"
        );
        for (m, c) in [(Some(501), None), (None, Some(-1))] {
            let e = sync_fees_claim(&st, user, m, c).await.unwrap_err();
            assert_eq!(e.0, StatusCode::BAD_REQUEST);
        }
        assert_eq!(st.broker.lock().await.account_fees(user), f(15, 7));
        let js = st.journals.lock().await;
        let op = js[&user]
            .events()
            .iter()
            .find(|e| matches!(e.kind(), journal::EventKind::FeesSet { .. }))
            .and_then(|e| e.origin().operator().cloned())
            .expect("запись с основанием");
        assert_eq!(op.who(), "cabinet-sso");
    }

    #[test]
    fn the_fee_claims_are_optional() {
        let c: sso::Claims = serde_json::from_str(r#"{"sub":"u","tenant":"t","jti":"j"}"#).unwrap();
        assert_eq!((c.markup_bps, c.commission_bps), (None, None));
        let c: sso::Claims = serde_json::from_str(
            r#"{"sub":"u","tenant":"t","jti":"j","markup_bps":12,"commission_bps":3}"#,
        )
        .unwrap();
        assert_eq!((c.markup_bps, c.commission_bps), (Some(12), Some(3)));
    }

    #[tokio::test]
    async fn the_monitor_charges_fees_on_pending_fill_and_stop_loss() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());
        price(&st, 1, 5_000_000).await;
        let (s, _) = call(
            &app,
            &uri("deposit"),
            Some(TOKEN),
            body(100_000_000, "seed-fm"),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let (s, _) = call(
            &app,
            &fees_uri("real"),
            Some(TOKEN),
            fees_body(10.into(), 10.into()),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let pending = serde_json::json!({"instrument":1,"side":"buy","qty":1000,"price":4_900_000,"sl":4_700_000});
        let (s, v) = send(&app, "POST", "/pending", pending).await;
        assert_eq!(s, StatusCode::OK, "{v}");

        price(&st, 1, 4_800_000).await;
        monitor_tick(&st).await;
        let id = {
            let b = st.broker.lock().await;
            let p = b.positions(DEFAULT_USER);
            assert_eq!(p.len(), 1, "ордер исполнен");
            assert_eq!(p[0].entry, broker::apply_markup(true, 4_800_000, 10));
            assert!(p[0].fees.commission > 0, "комиссия открытия");
            p[0].id
        };
        price(&st, 1, 4_650_000).await;
        monitor_tick(&st).await;
        assert!(st.broker.lock().await.positions(DEFAULT_USER).is_empty());
        {
            let js = st.journals.lock().await;
            let charged: Vec<_> = js[&DEFAULT_USER]
                .events()
                .iter()
                .filter(|e| {
                    matches!(e.kind(), journal::EventKind::CommissionCharged { trade, .. } if *trade == id)
                })
                .map(|e| e.origin().clone())
                .collect();
            assert_eq!(charged.len(), 2, "открытие и стоп-лосс");
            assert!(charged.iter().all(|o| o == &journal::Origin::System));
        }
        assert!(st.quarantined.lock().await.is_empty(), "сверка сошлась");
    }

    fn stops_uri() -> String {
        format!("/internal/sites/{TENANT}/settings/stops")
    }

    async fn put_stops(
        app: &Router,
        token: Option<&str>,
        b: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let mut req = Request::builder()
            .method("PUT")
            .uri(stops_uri())
            .header("content-type", "application/json");
        if let Some(t) = token {
            req = req.header(TOKEN_HEADER, t);
        }
        let resp = app
            .clone()
            .oneshot(
                req.body(Body::from(serde_json::to_vec(&b).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let s = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            s,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    #[tokio::test]
    async fn the_stops_setting_is_behind_the_secret_validates_and_is_idempotent() {
        let app = router(state(None));
        let (s, _) = put_stops(&app, Some(TOKEN), serde_json::json!({"min_stop_bps": 50})).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "секрет не задан — ручки нет");

        let st = state(Some(TOKEN));
        let app = router(st.clone());
        let (s, _) = put_stops(&app, None, serde_json::json!({"min_stop_bps": 50})).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, _) = get(&app, &stops_uri(), None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, _) = put_stops(&app, Some("wrong"), serde_json::json!({"min_stop_bps": 50})).await;
        assert_eq!(s, StatusCode::FORBIDDEN);

        let (s, v) = get(&app, &stops_uri(), Some(TOKEN)).await;
        assert_eq!(
            (s, v),
            (StatusCode::OK, serde_json::json!({"min_stop_bps": 0}))
        );

        for bad in [
            serde_json::json!({"min_stop_bps": -1}),
            serde_json::json!({"min_stop_bps": 10_001}),
            serde_json::json!({"min_stop_bps": 12.5}),
            serde_json::json!({"min_stop_bps": "50"}),
            serde_json::json!({"min_stop_bps": null}),
            serde_json::json!({}),
        ] {
            let (s, v) = put_stops(&app, Some(TOKEN), bad.clone()).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}");
            assert!(
                v["error"] == "invalid_min_stop_bps" || v["error"] == "bad_request",
                "{bad}: {v}"
            );
        }

        for _ in 0..2 {
            let (s, v) =
                put_stops(&app, Some(TOKEN), serde_json::json!({"min_stop_bps": 50})).await;
            assert_eq!(
                (s, v),
                (StatusCode::OK, serde_json::json!({"min_stop_bps": 50}))
            );
        }
        let (_, v) = get(&app, &stops_uri(), Some(TOKEN)).await;
        assert_eq!(v, serde_json::json!({"min_stop_bps": 50}));
        for edge in [0, 10_000] {
            let (s, v) =
                put_stops(&app, Some(TOKEN), serde_json::json!({"min_stop_bps": edge})).await;
            assert_eq!(
                (s, v),
                (StatusCode::OK, serde_json::json!({"min_stop_bps": edge}))
            );
        }
        assert_eq!(st.stops.for_tenant(Some("other-site")).await, 0);
        assert!(
            st.journals.lock().await.is_empty(),
            "журналы счетов не тронуты"
        );
    }

    #[tokio::test]
    async fn the_stops_setting_survives_a_restart() {
        let store = linked();
        let app = router(state_with(store.clone(), Some(TOKEN)));
        let (s, _) = put_stops(&app, Some(TOKEN), serde_json::json!({"min_stop_bps": 75})).await;
        assert_eq!(s, StatusCode::OK);

        let st2 = state_with(store, Some(TOKEN));
        restore_state(&st2).await.unwrap();
        assert_eq!(st2.stops.for_tenant(Some(TENANT)).await, 75);
        let (_, v) = get(&router(st2), &stops_uri(), Some(TOKEN)).await;
        assert_eq!(v, serde_json::json!({"min_stop_bps": 75}));
    }

    #[tokio::test]
    async fn stops_closer_than_the_minimum_are_refused_by_the_gateway() {
        let mut st = state(Some(TOKEN));
        st.stops = Arc::new(super::super::stops::StopSettings::new(100));
        let app = router(st.clone());
        price(&st, 1, 4_800_000).await;

        let body = |side: &str, sl: Option<i64>, tp: Option<i64>| serde_json::json!({"instrument": 1, "side": side, "qty": 1000, "sl": sl, "tp": tp});
        let (s, v) = send(&app, "POST", "/deals", body("buy", Some(4_760_000), None)).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
        assert_eq!(v["error"], "stop_loss_too_close");
        assert_eq!(v["min_stop_bps"], 100);
        assert_eq!(v["min_distance"], 48_000);
        assert_eq!(v["limit"], 4_752_000);
        assert!(v["detail"].as_str().unwrap().contains("47520.00"), "{v}");

        let (s, v) = send(&app, "POST", "/deals", body("buy", None, Some(4_840_000))).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(v["error"], "take_profit_too_close");
        let (s, v) = send(&app, "POST", "/deals", body("sell", Some(4_840_000), None)).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(v["limit"], 4_848_000);
        let (s, v) = send(&app, "POST", "/deals", body("sell", None, Some(4_760_000))).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(v["error"], "take_profit_too_close");
        assert!(
            st.broker.lock().await.positions(DEFAULT_USER).is_empty(),
            "ни одна не открыта"
        );

        let (s, v) = send(
            &app,
            "POST",
            "/deals",
            body("buy", Some(4_752_000), Some(4_848_000)),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let (s, _) = send(&app, "POST", "/deals", body("buy", None, None)).await;
        assert_eq!(s, StatusCode::OK);

        let limit = |sl: i64, tp: i64| serde_json::json!({"instrument": 1, "side": "buy", "qty": 1000, "price": 4_000_000, "sl": sl, "tp": tp});
        let (s, v) = send(&app, "POST", "/pending", limit(3_970_000, 4_040_000)).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(v["error"], "stop_loss_too_close");
        let (s, v) = send(&app, "POST", "/pending", limit(3_960_000, 4_040_000)).await;
        assert_eq!(s, StatusCode::OK, "{v}");

        st.stops = Arc::new(super::super::stops::StopSettings::new(0));
        let app = router(st.clone());
        let (s, _) = send(
            &app,
            "POST",
            "/deals",
            body("buy", Some(4_799_000), Some(4_801_000)),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
    }

    #[tokio::test]
    async fn the_account_tells_the_ticket_the_minimum() {
        let mut st = state(Some(TOKEN));
        st.stops = Arc::new(super::super::stops::StopSettings::new(40));
        let (s, v) = get(&router(st), "/account", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["min_stop_bps"], 40, "{v}");
    }
}
