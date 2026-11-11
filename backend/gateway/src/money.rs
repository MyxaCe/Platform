//! # money — внутренние денежные ручки (пополнение и вывод от имени сайта)
//!
//! Деньги на счёт терминала приходят и уходят **по решению сайта**: сайт видит
//! крипто-депозит в тестовой сети или одобряет заявку на вывод и говорит об этом
//! терминалу сервер-сервер. Терминал остаётся владельцем денежного числа:
//! проводка идёт обычным путём мутации (журнал, инварианты, сверка, исходящая
//! очередь — одной транзакцией), отдельного пути у денег нет.
//!
//! ## Граница
//!
//! * `POST /internal/sites/{tenant}/users/{sub}/real/deposit`
//! * `POST /internal/sites/{tenant}/users/{sub}/real/withdraw`
//! * `POST /internal/sites/{tenant}/users/{sub}/{demo|real}/trading-freeze`
//! * `POST /internal/sites/{tenant}/users/{sub}/{demo|real}/leverage`
//! * `POST /internal/sites/{tenant}/users/{sub}/{demo|real}/fees`
//! * `GET  /internal/sites/{tenant}/users/{sub}/accounts`
//!
//! Человек адресуется той же парой, что и в SSO и на шине: сайт (`tenant`) и
//! пользователь сайта (`sub`); счёт — ещё и родом. У человека до двух счетов:
//! демо (бумажные деньги, стартовая сумма площадки) и реальный (открывается
//! нулём, деньги — только через эти ручки). Деньги ходят **только** на
//! реальный: у демо-счёта пополнения и вывода нет. Реальный счёт заводится
//! первым пополнением. Внутренний номер счёта наружу не уходит — вместо него
//! `account_id` (UUIDv5 от ключа счёта).
//!
//! ## Авторизация — fail-closed
//!
//! Заголовок `X-Internal-Token` с общим секретом из `INTERNAL_MONEY_TOKEN`,
//! сравнение за постоянное время. Секрет не задан — ручек **нет**: `404`, как
//! у несуществующего пути. Незаданный секрет не может означать «пускать всех».
//!
//! Наружу ручки не публикуются: nginx терминала проксирует на шлюз только явный
//! список префиксов, а `/internal` закрывает отдельно.
//!
//! ## Идемпотентность
//!
//! `external_ref` (хэш транзакции депозита, id заявки на вывод) записывается в
//! журнал как ссылка ручной проводки (`incident = "ext:<ref>"`) — то есть
//! хранится там же и той же транзакцией, что сами деньги, и переживает рестарт
//! без отдельной таблицы. Повтор с тем же ref и теми же параметрами — `200` с
//! `replayed: true` и номером ПЕРВОЙ записи, без второй проводки. Тот же ref с
//! другими параметрами (сумма, операция, счёт) — `409`: это уже не повтор, а
//! конфликт, и угадывать, какой из двух запросов правильный, мы не вправе.

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

/// Заголовок с общим секретом.
pub const TOKEN_HEADER: &str = "x-internal-token";
/// Префикс ссылки в журнале: отличает внешний ref от номера инцидента ручной
/// проводки центра управления (ADM-03), которая пишется в то же поле.
const REF_PREFIX: &str = "ext:";
const MAX_REF_LEN: usize = 200;
/// Короче — предупреждаем в логе старта (не отказываем: стенд демо).
const MIN_TOKEN_LEN: usize = 32;
/// Валюта счёта. Единственная в терминале — см. `bus::CURRENCY`.
const CURRENCY: &str = "USD";

/// Секрет из окружения. Пустой или из пробелов — то же, что не задан.
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
        // Деньги — ТОЛЬКО на реальном счёте: у демо-счёта пополнения и вывода
        // нет по определению (бумажные деньги).
        .route(
            "/internal/sites/:tenant/users/:sub/real/deposit",
            post(deposit),
        )
        .route(
            "/internal/sites/:tenant/users/:sub/real/withdraw",
            post(withdraw),
        )
        // Заморозка — на любом из двух счетов: `{kind}` = demo | real.
        .route(
            "/internal/sites/:tenant/users/:sub/:kind/trading-freeze",
            post(trading_freeze),
        )
        // Плечо счёта для новых позиций (Р-446) — тоже на любом из двух.
        .route(
            "/internal/sites/:tenant/users/:sub/:kind/leverage",
            post(set_leverage),
        )
        // Тариф счёта (наценка и комиссия, 10.11) — на любом из двух.
        .route(
            "/internal/sites/:tenant/users/:sub/:kind/fees",
            post(set_fees),
        )
        .route("/internal/sites/:tenant/users/:sub/accounts", get(accounts))
}

/// Сравнение за постоянное время: длительность не зависит от того, в каком
/// байте строки расходятся. Длина в сравнение входит, но цикл идёт по большей
/// из двух — ранний выход по длине подсказывал бы её.
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
        // Ручки выключены — отвечаем как несуществующий путь, без тела.
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
    /// Сумма в минимальных единицах валюты (центы), целое > 0.
    amount_minor: i64,
    currency: String,
    /// Ключ идемпотентности: хэш крипто-транзакции, id заявки на вывод.
    external_ref: String,
    /// Кто инициировал: сервис или оператор сайта. Ложится в `who` журнала.
    #[serde(alias = "operator")]
    initiator: String,
    /// Основание. Ложится в `reason` журнала.
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

/// Проверенный запрос: всё, что нужно для проводки.
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

/// Проводка с этим ref, если она уже есть: `(счёт, операция, сумма, seq)`.
///
/// Перебор журналов в памяти: на демо-объёмах это дешевле и надёжнее отдельного
/// индекса, который пришлось бы держать в согласии с журналом.
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
    // Авторизация — первой, до разбора тела: без секрета не отвечаем ничего,
    // что выдавало бы форму контракта.
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

    // Реальный счёт заводится по требованию — первым пополнением (идемпотентно:
    // маппинг найти-или-создать). Вывод счёта не заводит: выводить неоткуда.
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

    // Одна денежная операция за раз: «нет ли уже ref → провести → записать»
    // обязано быть неделимым, иначе два одновременных повтора зачислят дважды.
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
            // Вывод со счёта, которого ещё нет, — отказ, а не заведение счёта
            // ради отказа: `with_account` завёл бы его стартовой суммой.
            if !st.broker.lock().await.has_account(user) {
                return fail(StatusCode::NOT_FOUND, "account_not_opened", None);
            }
            let m = marks(&st).await;
            let mut free_seen: Option<Cents> = None;
            // Замороженный счёт не выводит: проверка под локом счёта, внутри.
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
        // Проводка записана в хранилище, но журнал в памяти её не принял
        // (счёт ушёл в карантин): номера нет, и выдумывать его нельзя.
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
        // Текущий остаток счёта. У повтора — тоже текущий, а не на момент
        // первой проводки: между ними могли быть сделки.
        "balance_minor": b.balance(user),
        "free_margin_minor": b.free_margin(user, &m),
        "seq": seq,
        "replayed": replayed,
    });
    (StatusCode::OK, Json(body)).into_response()
}

// ============================ Заморозка торговли ============================

#[derive(Deserialize)]
struct FreezeReq {
    frozen: bool,
    reason: String,
    #[serde(alias = "initiator")]
    operator: String,
    /// Необязательная ссылка на дело на стороне сайта (AML-кейс, тикет).
    #[serde(default)]
    external_ref: Option<String>,
}

/// `POST /internal/sites/{tenant}/users/{sub}/{kind}/trading-freeze`.
///
/// Заморозка — запись журнала `TradingFreezeSet` с оператором и основанием:
/// персистентна (лента переживает рестарт) и видна в журнале счёта. Повтор
/// того же состояния новой записи не пишет (`changed: false`).
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
    // Заморозить можно и счёт, который ещё не заводился: он заводится тем же
    // путём, что первой операцией, — иначе заморозка до первого входа
    // потерялась бы.
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

// ============================ Плечо счёта ===================================

#[derive(Deserialize)]
struct LeverageReq {
    /// Знаменатель 1:N, целое `1..=1000`. Итог портфеля / override клиента,
    /// посчитанный сайтом (Р-446).
    leverage: i64,
    reason: String,
    #[serde(alias = "initiator")]
    operator: String,
    /// Необязательная ссылка на дело на стороне сайта (заявка, запись журнала).
    #[serde(default)]
    external_ref: Option<String>,
}

/// Назначить счёту плечо одной записью журнала `LeverageSet` (через обычный
/// путь мутации: инварианты, сверка, слепок и лента одной транзакцией).
///
/// Отдаёт `(плечо до, сменилось ли)`. Плечо «до» — **действующее**: если
/// счёту плечо не назначали, это плечо площадки. Счёт, которого ещё нет в
/// памяти, заводится тем же путём, что первой операцией (TRM-46).
pub(crate) async fn apply_leverage(
    st: &AppState,
    user: UserId,
    lev: i128,
    operator: Operator,
) -> Result<(i128, bool), super::ApiErr> {
    with_account(st, user, Origin::Operator(operator), |b| {
        let before = b.account_leverage(user);
        b.set_account_leverage(user, lev)
            .map(|changed| (before, changed))
            .map_err(super::deal_error)
    })
    .await
}

/// `POST /internal/sites/{tenant}/users/{sub}/{kind}/leverage`.
///
/// Плечо **для новых позиций** счёта. Открытые позиции держат своё плечо
/// снимком. Повтор того же значения записи не пишет (`changed: false`).
///
/// Счёт, у которого нет даже маппинга личности (человек ни разу не входил в
/// терминал этим родом счёта), — `404 account_not_opened`: заводить счёт ради
/// плеча незачем, своё плечо он получит claim-ом `leverage` при первом входе.
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

    let (previous, changed) = match apply_leverage(&st, user, lev, operator).await {
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
            "[money] плечо счёта {} ({tenant}/{sub}/{}) 1:{previous} → 1:{lev} оператором {}: {}",
            user.0,
            kind.tag(),
            req.operator.trim(),
            req.reason.trim()
        );
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "tenant": tenant, "sub": sub, "account_kind": kind.tag(),
            "account_id": bus::account_uuid(&tenant, &sub, kind),
            "leverage": lev, "previous_leverage": previous,
            "changed": changed, "seq": seq,
        })),
    )
        .into_response()
}

// ============================ Тариф счёта ===================================

#[derive(Deserialize)]
struct FeesReq {
    /// Наценка на спред, целые базисные пункты `0..=MAX_FEE_BPS` (Standard).
    markup_bps: i64,
    /// Комиссия от нотионала сделки, целые bps `0..=MAX_FEE_BPS` (Pro / ENC Prime).
    commission_bps: i64,
    reason: String,
    #[serde(alias = "initiator")]
    operator: String,
    /// Необязательная ссылка на дело на стороне сайта (заявка, запись журнала).
    #[serde(default)]
    external_ref: Option<String>,
}

/// Назначить счёту тариф одной записью журнала `FeesSet` (обычный путь
/// мутации: инварианты, сверка, слепок и лента одной транзакцией).
///
/// Отдаёт `(тариф до, сменился ли)`. Счёт, которого ещё нет в памяти,
/// заводится тем же путём, что первой операцией (TRM-46).
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

/// `POST /internal/sites/{tenant}/users/{sub}/{kind}/fees`.
///
/// Тариф **для новых позиций** счёта (решение владельца 10.11): наценка на
/// спред и комиссия за сделку. Открытые позиции закрываются по своему снимку.
/// Повтор того же тарифа записи не пишет (`changed: false`) — маршрут
/// идемпотентен по значению.
///
/// Счёт без маппинга личности — `404 account_not_opened`: тариф он получит
/// claim-ами `markup_bps`/`commission_bps` при первом входе.
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

// ============================ Список счетов =================================

/// `GET /internal/sites/{tenant}/users/{sub}/accounts` — счета человека.
///
/// Только чтение: ничего не заводит. Счёт, которого нет, в списке отсутствует;
/// счёт, у которого есть маппинг, но ещё нет ни одной записи (`opened: false`),
/// — с `null` вместо денег, как в `/account` (денег у него нет, а не ноль).
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
            // Плечо, с которым откроется СЛЕДУЮЩАЯ позиция: своё плечо счёта
            // или, если не назначали, плечо площадки.
            "leverage": b.account_leverage(user),
            "leverage_assigned": b.assigned_leverage(user).is_some(),
            // Тариф для НОВЫХ позиций (10.11): нули — без тарифа.
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

    /// Хранилище с настоящим маппингом личностей: `MemoryStore` его не ведёт,
    /// а денежные ручки адресуют счёт именно парой `(tenant, sub)`.
    ///
    /// Номера раздаются с 1 — первый же человек получает `DEFAULT_USER`, и
    /// позиции на его счёт открываются обычной ручкой `/deals` (SSO в тестах
    /// выключен, и она работает с `DEFAULT_USER`).
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
        // Стенд демо: торговля на реальном счёте открыта, события реальных
        // счетов публикуются. Выключенные ворота проверяются отдельно.
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

    // ---- Авторизация -------------------------------------------------------

    #[tokio::test]
    async fn without_the_env_secret_the_routes_do_not_exist() {
        let app = router(state(None));
        for op in ["deposit", "withdraw"] {
            // Даже с «каким-то» токеном: секрета нет — ручек нет.
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

        // Совпадающий префикс — не совпадение.
        let prefix = &TOKEN[..TOKEN.len() - 1];
        let (s, _) = call(&app, &uri("deposit"), Some(prefix), body(100, "tx-1")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);

        // Ни один отказ не провёл денег.
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

    // ---- Пополнение и идемпотентность --------------------------------------

    #[tokio::test]
    async fn a_deposit_credits_once_per_external_ref() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());

        let (s, first) = call(&app, &uri("deposit"), Some(TOKEN), body(25_000, "0xabc")).await;
        assert_eq!(s, StatusCode::OK, "{first}");
        let start = 0; // реальный счёт открывается нулём
        assert_eq!(first["balance_minor"], serde_json::json!(start + 25_000));
        assert_eq!(first["replayed"], false);
        let seq = first["seq"].as_u64().expect("seq");
        assert!(seq >= 2, "до пополнения в журнале заведение счёта: {first}");

        // Повтор — тот же ответ по сути, второй проводки нет.
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

        // Тот же ref с другой суммой — конфликт, не повтор.
        let (s, v) = call(&app, &uri("deposit"), Some(TOKEN), body(99, "0xabc")).await;
        assert_eq!(s, StatusCode::CONFLICT, "{v}");
        assert_eq!(v["error"], "external_ref_conflict");
        // И как вывод — тоже конфликт.
        let (s, _) = call(&app, &uri("withdraw"), Some(TOKEN), body(25_000, "0xabc")).await;
        assert_eq!(s, StatusCode::CONFLICT);
        // И на чужой счёт.
        let (s, _) = call(
            &app,
            &format!("/internal/sites/{TENANT}/users/someone-else/real/deposit"),
            Some(TOKEN),
            body(25_000, "0xabc"),
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT);

        // Другой ref — новая проводка.
        let (s, v) = call(&app, &uri("deposit"), Some(TOKEN), body(1, "0xdef")).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["balance_minor"], serde_json::json!(start + 25_001));
    }

    /// Идемпотентность держится на журнале, а не на памяти процесса: после
    /// рестарта повтор того же крипто-депозита не зачисляется.
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
        // `operator` — допустимое имя поля вместо `initiator`.
        let (s, v) = call(
            &app,
            &uri("deposit"),
            Some(TOKEN),
            serde_json::json!({"amount_minor":1,"currency":"usd","external_ref":"r-op","operator":"ops@site","reason":"y"}),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }

    // ---- Вывод -------------------------------------------------------------

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
        price(&st, 1, 5_000_000).await; // BTC $50 000.00, pd=2 qd=5
        let app = router(st.clone());

        // Заводим счёт пополнением — тем же путём, каким его заведёт сайт.
        let (s, _) = call(&app, &uri("deposit"), Some(TOKEN), body(10_000_000, "seed")).await;
        assert_eq!(s, StatusCode::OK);
        let balance = st.broker.lock().await.balance(DEFAULT_USER).unwrap();

        // Позиция на половину остатка (плечо 1: маржа = нотионал).
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

        // Отказанный ref не «сгорел»: тот же ref с допустимой суммой проходит.
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

    // ---- Шина --------------------------------------------------------------

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

        // Повтор события не порождает.
        let before = balance_changes(&queued(&st).await).len();
        let (s, _) = call(&app, &uri("deposit"), Some(TOKEN), body(10_000, "tx-bus")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(balance_changes(&queued(&st).await).len(), before);
    }

    // ---- Плечо: колл и стоп-аут вживую ------------------------------------

    /// Сценарий демо целиком: деньги пришли пополнением, позиция при плече 20
    /// почти на весь остаток, рынок против на 1 % — предупреждение, ещё на 2 %
    /// — принудительное закрытие. Числа выведены руками из входных данных.
    ///
    /// При полном использовании остатка под маржу и плече 20 движение против
    /// на `x %` съедает `20·x %` залога: колл (100 %) — почти сразу, стоп-аут
    /// (50 %) — на ~2,5 %. Здесь маржа 95 % остатка: на −1 % уровень 85 %,
    /// на −3 % — 45 %.
    #[tokio::test]
    async fn leverage_20_margin_call_then_stop_out() {
        let st = state(Some(TOKEN));
        let app = router(st.clone());
        const PX: i64 = 5_000_000; // BTC $50 000.00, pd=2 qd=5
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

        // Нотионал = 19 остатков: при плече 20 маржа — 95 % остатка.
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

        // При плече 1 такую позицию не пускают — плечо действительно решает.
        assert_eq!(
            app.clone().oneshot(deal(qty)).await.unwrap().status(),
            StatusCode::PAYMENT_REQUIRED
        );

        // Плечо 20 и пороги, как на стенде. Счёт переносим как есть.
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

        // −1 %: уровень ~85 % — предупреждение, позиция жива.
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

        // −3 %: уровень ~45 % < 50 — принудительное закрытие.
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

    // ---- Гонка повторов ----------------------------------------------------

    /// Одновременные запросы с одним ref (сайт повторил по таймауту, пока
    /// первый ещё шёл) — зачисление ровно одно.
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
        let start = 0; // реальный счёт открывается нулём
        assert_eq!(
            st.broker.lock().await.balance(DEFAULT_USER),
            Some(start + 123_456)
        );
    }

    // ---- Заморозка ---------------------------------------------------------

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

    /// Замороженный счёт: открыть, поставить ордер, вывести, сбросить —
    /// нельзя; закрыть, пополнить — можно; SL срабатывает. Разморозка
    /// возвращает торговлю.
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

        // Две позиции до заморозки: одну закроем руками, вторую закроет SL.
        let (s, v) = send(&app, "POST", "/deals", deal(None)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let (s, v) = send(&app, "POST", "/deals", deal(Some(4_900_000))).await;
        assert_eq!(s, StatusCode::OK, "{v}");

        let (s, v) = call(&app, &uri("trading-freeze"), Some(TOKEN), freeze_body(true)).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["changed"], true);
        assert_eq!(v["frozen"], true);
        let frozen_seq = v["seq"].clone();

        // Повтор — без новой записи.
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

        // SL работает как обычно.
        price(&st, 1, 4_800_000).await;
        monitor_tick(&st).await;
        assert!(st.broker.lock().await.positions(DEFAULT_USER).is_empty());

        let (_, acc) = send(&app, "GET", "/account", serde_json::Value::Null).await;
        assert_eq!(acc["trading_frozen"], true, "{acc}");

        // Журнал: запись с оператором, основанием и делом.
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

    /// Заморозка живёт в журнале и переживает рестарт шлюза.
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

    // ---- Демо и реальный счёт ----------------------------------------------

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

    /// Пополнение заводит РЕАЛЬНЫЙ счёт с нуля; демо-счёт человека — другой
    /// счёт со своим номером, своим `accountId` и своими деньгами.
    #[tokio::test]
    async fn money_lands_on_the_real_account_only() {
        let store = linked();
        let st = state_with(store.clone(), Some(TOKEN));
        let app = router(st.clone());

        // До первого пополнения счетов нет — список пуст, ничего не заведено.
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

        // Демо-счёт того же человека — отдельный номер; деньги на нём не те.
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

        // У демо-счёта денежных ручек нет вовсе.
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
        // Список — под тем же секретом.
        let (s, _) = get(&app, &accounts_uri(), None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    /// События несут род и идентификатор счёта; у реального — свой ключ
    /// `account.opened`, иначе дедуп потребителя склеил бы его с демо.
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

    /// Пока потребитель не различает род счёта, события реального счёта не
    /// собираются вовсе: иначе они легли бы в строку демо-счёта.
    #[tokio::test]
    async fn real_account_events_stay_home_until_enabled() {
        let mut st = state(Some(TOKEN));
        st.bus_real_events = false;
        let app = router(st.clone());
        let (s, _) = call(&app, &uri("deposit"), Some(TOKEN), body(700, "tx-quiet")).await;
        assert_eq!(s, StatusCode::OK);
        assert!(queued(&st).await.is_empty(), "реальный счёт молчит на шине");
    }

    /// Ворота реальной торговли закрыты, пока площадка их явно не открыла.
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

    /// `/account` называет род счёта — по нему фронт рисует бейдж.
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

    // ---- Плечо счёта (Р-446) -----------------------------------------------

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

        // Счёт заводим пополнением — дальше проверяем только тело.
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

        // Ни один отказ ничего не записал.
        assert_eq!(st.broker.lock().await.assigned_leverage(DEFAULT_USER), None);

        // Человек, ни разу не входивший демо-счётом: счёта нет, и ради плеча
        // он не заводится — плечо приедет claim-ом при первом входе.
        let (s, v) = call(&app, &lev_uri("demo"), Some(TOKEN), lev_body(20.into())).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
        assert_eq!(v["error"], "account_not_opened");
    }

    /// Плечо из маршрута: журналируется с оператором, достаётся только новым
    /// позициям, повтор не пишет запись, рестарт переживает (свёртка ленты
    /// сходится со слепком), видно в `/account` и в списке счетов.
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

        // Рестарт: лента и слепок из того же хранилища.
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

    /// Дыра заморозки: ордер, поставленный ДО заморозки, при срабатывании не
    /// открывает позицию — монитор снимает его записью с причиной.
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

    /// Незамороженный счёт — тот же ордер исполняется (контроль: отказ выше
    /// не случайность триггера).
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

    /// Claim `leverage` токена входа: применяется записью журнала, повтор
    /// молчит, вне диапазона — отказ входа, без claim-а — ничего.
    #[tokio::test]
    async fn the_sso_leverage_claim_syncs_the_account() {
        let st = state(Some(TOKEN));
        let user = st
            .store
            .resolve_account(TENANT, SUB, AccountKind::Demo)
            .await
            .unwrap();
        sync_leverage_claim(&st, user, None).await.unwrap();
        assert!(
            !st.broker.lock().await.has_account(user),
            "без claim-а ничего"
        );

        sync_leverage_claim(&st, user, Some(10)).await.unwrap();
        assert_eq!(st.broker.lock().await.assigned_leverage(user), Some(10));
        let n = st.journals.lock().await[&user].len();
        sync_leverage_claim(&st, user, Some(10)).await.unwrap();
        assert_eq!(st.journals.lock().await[&user].len(), n, "повтор не пишет");

        for bad in [0, 1001] {
            let e = sync_leverage_claim(&st, user, Some(bad)).await.unwrap_err();
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

    // ---- Тариф счёта: наценка и комиссия (решение 10.11) --------------------

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

    /// Тариф из маршрута: журналируется с оператором, повтор не пишет, виден
    /// в `/account` и в списке счетов; новая позиция исполняется с наценкой
    /// и платит комиссию на открытии и закрытии; старая — по своему снимку;
    /// событие сделки на шине несёт доход площадки; рестарт сходится.
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

        // Позиция ДО тарифа — без наценки и комиссии.
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

        // Закрытие обеих: старая — без тарифа (снимок), новая — с наценкой
        // и комиссией закрытия.
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
        // Ответ — от новых к старым.
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

        // Рестарт: лента и слепок из того же хранилища — сверка сходится.
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

    /// Комиссия открытия двигает остаток — событие баланса уходит с причиной
    /// `trade`, а не без причины.
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

    /// Claims тарифа токена входа: применяются записью журнала, повтор молчит,
    /// вне диапазона — отказ входа, один claim не трогает другую ставку.
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

    /// Монитор: отложенный ордер исполняется с наценкой и комиссией тарифа,
    /// стоп-лосс закрывает с комиссией — записями системы, сверка сходится.
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

    // ---- Минимум стопов площадки (settings/stops) -------------------------

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

        // До первой записи — умолчание окружения (в тестах MIN_STOP_BPS не задан).
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
        // Границы диапазона допустимы.
        for edge in [0, 10_000] {
            let (s, v) =
                put_stops(&app, Some(TOKEN), serde_json::json!({"min_stop_bps": edge})).await;
            assert_eq!(
                (s, v),
                (StatusCode::OK, serde_json::json!({"min_stop_bps": edge}))
            );
        }
        // Чужая площадка своего значения не получила.
        assert_eq!(st.stops.for_tenant(Some("other-site")).await, 0);
        // Настройка — не событие счёта: ни одной записи журнала.
        assert!(
            st.journals.lock().await.is_empty(),
            "журналы счетов не тронуты"
        );
    }

    /// Значение площадки переживает рестарт: оно в хранилище, а не только в памяти.
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

    /// Граница на сервере: ближе минимума — `422` с причиной и числами, на
    /// рыночном и на лимитном ордере, для покупки и для продажи.
    #[tokio::test]
    async fn stops_closer_than_the_minimum_are_refused_by_the_gateway() {
        let mut st = state(Some(TOKEN));
        // Без площадки (SSO в тестах выключен) действует умолчание.
        st.stops = Arc::new(super::super::stops::StopSettings::new(100));
        let app = router(st.clone());
        // ask = bid = 4 800 000 → 1 % = 48 000.
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

        // Ровно на минимуме — можно; без уровней — можно.
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

        // Лимитный: от цены ордера (4 000 000 → 40 000), а не от рынка.
        let limit = |sl: i64, tp: i64| serde_json::json!({"instrument": 1, "side": "buy", "qty": 1000, "price": 4_000_000, "sl": sl, "tp": tp});
        let (s, v) = send(&app, "POST", "/pending", limit(3_970_000, 4_040_000)).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(v["error"], "stop_loss_too_close");
        let (s, v) = send(&app, "POST", "/pending", limit(3_960_000, 4_040_000)).await;
        assert_eq!(s, StatusCode::OK, "{v}");

        // Ноль — ограничения нет.
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

    /// `/account` отдаёт минимум стопов — по нему тикет подставляет уровни.
    #[tokio::test]
    async fn the_account_tells_the_ticket_the_minimum() {
        let mut st = state(Some(TOKEN));
        st.stops = Arc::new(super::super::stops::StopSettings::new(40));
        let (s, v) = get(&router(st), "/account", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["min_stop_bps"], 40, "{v}");
    }
}
