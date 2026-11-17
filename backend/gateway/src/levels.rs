//! # levels — изменение SL/TP у открытой позиции и отложенного ордера (17.11)
//!
//! * `PUT /deals/{id}/levels`   — уровни открытой позиции;
//! * `PUT /pending/{id}/levels` — уровни отложенного ордера.
//!
//! Тело: `{"sl": число | null, "tp": число | null}` в сырых единицах цены
//! инструмента. Число — поставить или сдвинуть, `null` — снять, поля нет —
//! оставить как есть. Хотя бы одно поле обязано быть.
//!
//! Ответ `200 {"id", "sl", "tp", "changed"}` — какими уровни стали; `changed:
//! false` — они уже были такими, записи в журнале нет (повтор безопасен).
//!
//! ## Отказы
//!
//! | код | `error` | когда |
//! |---|---|---|
//! | 409 | `sltp_edit_disabled` | запись выключена флагом (`SLTP_EDIT_ENABLED` не `1`) |
//! | 400 | `bad_request` / `levels_required` / `invalid_level` | тело не разобрано / нет ни одного поля / уровень не положительный |
//! | 404 | `unknown_position` / `unknown_pending` | нет такой позиции/ордера на СВОЁМ счёте (в т.ч. уже закрыта стопом) |
//! | 422 | `stop_loss_too_close` / `take_profit_too_close` | ближе минимума площадки; `limit`, `min_distance`, `reference`, `min_stop_bps` — как у `POST /deals` |
//! | 409 | `stop_loss_already_reached` / `take_profit_already_reached` | уровень уже «пробит» — сработал бы на ближайшем тике |
//! | 403 | `account_frozen` | торговля заморожена оператором |
//! | 423 | `account_quarantined` | счёт в карантине |
//! | 403 | `identity_not_verified` | реальный счёт при закрытой торговле на реальных |
//! | 503 | `no price yet` | у инструмента позиции ещё нет котировки |
//!
//! ## Цена отсчёта
//!
//! У позиции — **рыночная цена закрытия**: bid для покупки, ask для продажи.
//! От неё считается минимум площадки, и уровень не должен быть уже пробит ни
//! ею, ни марк-ценой, по которой идёт прогон триггеров. У ордера — его
//! собственная цена (позиция родится там), как при постановке.
//!
//! Проверяется только **изменённый** уровень: оставленный как был человек не
//! ставит заново.
//!
//! ## Выкат в два шага
//!
//! Запись нового вида (`position_levels_changed`, `pending_levels_changed`)
//! прежний образ не разберёт. Поэтому **чтение** (свёртка) в этой сборке есть
//! всегда, а **запись** — только под `SLTP_EDIT_ENABLED=1`. Шаг 1: выкатить с
//! флагом 0 — откат на прежний образ ещё свободен, записей нового вида нет.
//! Шаг 2: включить флаг. С первой записи откат возможен только на сборку не
//! старше этой (с любым значением флага).

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Deserializer, Serialize};

use broker::journal::Origin;
use broker::PosSide;
use domain::account::AccountKind;

use super::{
    deal_error, err, require_principal, stops, stops_err, with_account_gated, ApiErr, AppState,
    ErrResp, Gate,
};

/// Флаг записи из окружения: `1` или `true` — включено, всё прочее — нет.
pub(crate) fn enabled_from_env(raw: Option<&str>) -> bool {
    matches!(raw.map(str::trim), Some("1") | Some("true"))
}

/// Поле, которое могло не прийти вовсе: `None` — не пришло (оставить),
/// `Some(None)` — пришёл `null` (снять), `Some(Some(v))` — поставить.
fn present<'de, D>(d: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<i64>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
pub(crate) struct LevelsReq {
    #[serde(default, deserialize_with = "present")]
    sl: Option<Option<i64>>,
    #[serde(default, deserialize_with = "present")]
    tp: Option<Option<i64>>,
}

#[derive(Serialize)]
pub(crate) struct LevelsDto {
    id: u64,
    sl: Option<i64>,
    tp: Option<i64>,
    changed: bool,
}

fn disabled() -> ApiErr {
    (
        StatusCode::CONFLICT,
        Json(ErrResp {
            error: "sltp_edit_disabled".to_string(),
            extra: Some(serde_json::json!({
                "detail": "changing SL/TP of an open position or order is not enabled on this server yet"
            })),
        }),
    )
}

/// Общее начало обеих ручек: личность, флаг, тело, ворота реального счёта.
async fn prologue(
    st: &AppState,
    headers: &HeaderMap,
    body: Result<Json<LevelsReq>, JsonRejection>,
) -> Result<(super::Principal, LevelsReq), ApiErr> {
    let who = require_principal(st, headers).await?;
    if !st.sltp_edit_enabled {
        return Err(disabled());
    }
    let req = match body {
        Ok(Json(r)) => r,
        Err(rej) => {
            let mut e = err(StatusCode::BAD_REQUEST, "bad_request");
            e.1 .0.extra = Some(serde_json::json!({ "detail": rej.body_text() }));
            return Err(e);
        }
    };
    if req.sl.is_none() && req.tp.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "levels_required"));
    }
    let positive = |v: Option<Option<i64>>| !matches!(v, Some(Some(x)) if x <= 0);
    if !positive(req.sl) || !positive(req.tp) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_level"));
    }
    // Те же ворота, что у открытия: реальными средствами не торгуют, пока
    // личность не проверена (TRM-44).
    if st.broker.lock().await.snapshot(who.user).profile.kind == AccountKind::Real
        && !st.real_trading_enabled
    {
        return Err(err(StatusCode::FORBIDDEN, "identity_not_verified"));
    }
    Ok((who, req))
}

/// Уровень, который проверяется: только изменённый и только поставленный.
fn changed_level(new: Option<i64>, cur: Option<i64>) -> Option<i64> {
    if new != cur {
        new
    } else {
        None
    }
}

/// `PUT /deals/{id}/levels`.
pub(crate) async fn edit_deal_levels(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    body: Result<Json<LevelsReq>, JsonRejection>,
) -> Result<Json<LevelsDto>, ApiErr> {
    let (who, req) = prologue(&st, &headers, body).await?;
    let user = who.user;
    // Позиция ищется ДО операции: правка несуществующей позиции не должна
    // заводить счёт (`with_account_gated` заводит его первым делом).
    let instrument = st
        .broker
        .lock()
        .await
        .positions(user)
        .into_iter()
        .find(|p| p.id == id)
        .map(|p| p.instrument)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown_position"))?;
    // Цены — до лока счёта, как у открытия: тикеры под своим локом.
    let (bid, ask, mark) = {
        let tickers = st.tickers.lock().await;
        let t = tickers
            .get(&instrument)
            .filter(|t| t.bid > 0 && t.ask > 0)
            .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "no price yet"))?;
        (t.bid, t.ask, t.last)
    };
    let min_bps = st.stops.for_tenant(who.tenant.as_deref()).await;
    let (sl, tp, changed) = with_account_gated(&st, user, Origin::Client, Gate::NotFrozen, |b| {
        // Под локом — заново: между поиском и этим местом позицию мог
        // закрыть стоп. Тогда это «нет такой позиции», а не правка закрытой.
        let cur = b
            .positions(user)
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown_position"))?;
        let sl = req.sl.unwrap_or(cur.sl);
        let tp = req.tp.unwrap_or(cur.tp);
        // Цена закрытия стороны: покупку закрывают по bid, продажу — по ask.
        let quote = match cur.side {
            PosSide::Long => bid,
            PosSide::Short => ask,
        };
        stops::check(
            cur.side,
            quote,
            changed_level(sl, cur.sl),
            changed_level(tp, cur.tp),
            min_bps,
        )
        .map_err(|e| stops_err(e, cur.pd))?;
        let at: Vec<i64> = [quote, mark].into_iter().filter(|m| *m > 0).collect();
        let changed = b
            .set_position_levels(user, id, sl, tp, &at)
            .map_err(deal_error)?;
        Ok((sl, tp, changed))
    })
    .await?;
    Ok(Json(LevelsDto {
        id,
        sl,
        tp,
        changed,
    }))
}

/// `PUT /pending/{id}/levels`.
pub(crate) async fn edit_pending_levels(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    body: Result<Json<LevelsReq>, JsonRejection>,
) -> Result<Json<LevelsDto>, ApiErr> {
    let (who, req) = prologue(&st, &headers, body).await?;
    let user = who.user;
    if !st
        .broker
        .lock()
        .await
        .pendings(user)
        .iter()
        .any(|o| o.id == id)
    {
        return Err(err(StatusCode::NOT_FOUND, "unknown_pending"));
    }
    let min_bps = st.stops.for_tenant(who.tenant.as_deref()).await;
    let (sl, tp, changed) = with_account_gated(&st, user, Origin::Client, Gate::NotFrozen, |b| {
        let cur = b
            .pendings(user)
            .into_iter()
            .find(|o| o.id == id)
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "unknown_pending"))?;
        let sl = req.sl.unwrap_or(cur.sl);
        let tp = req.tp.unwrap_or(cur.tp);
        // У ордера отсчёт — от его цены, как при постановке.
        stops::check(
            cur.side,
            cur.price,
            changed_level(sl, cur.sl),
            changed_level(tp, cur.tp),
            min_bps,
        )
        .map_err(|e| stops_err(e, cur.pd))?;
        let changed = b.set_pending_levels(user, id, sl, tp).map_err(deal_error)?;
        Ok((sl, tp, changed))
    })
    .await?;
    Ok(Json(LevelsDto {
        id,
        sl,
        tp,
        changed,
    }))
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn flag_is_off_unless_explicitly_on() {
        assert!(!enabled_from_env(None));
        assert!(!enabled_from_env(Some("")));
        assert!(!enabled_from_env(Some("0")));
        assert!(!enabled_from_env(Some("yes")));
        assert!(enabled_from_env(Some("1")));
        assert!(enabled_from_env(Some(" true ")));
    }

    #[test]
    fn absent_null_and_number_are_three_different_things() {
        let r: LevelsReq = serde_json::from_str(r#"{"sl": null, "tp": 5}"#).unwrap();
        assert_eq!((r.sl, r.tp), (Some(None), Some(Some(5))));
        let r: LevelsReq = serde_json::from_str(r#"{"tp": 5}"#).unwrap();
        assert_eq!((r.sl, r.tp), (None, Some(Some(5))));
        assert!(serde_json::from_str::<LevelsReq>(r#"{"sl": 1.5}"#).is_err());
    }
}

// ============ Проводка: через роутер, монитор и хранилище ===================
#[cfg(test)]
mod wiring {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    use broker::journal::{EventKind, Origin};

    use crate::{
        build_state, build_state_with, feed, monitor_tick, restore_state, router, stops, AppState,
        DEFAULT_USER,
    };

    pub(crate) async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
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
        let v = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, v)
    }

    /// Котировка инструмента 1: bid / ask / last раздельно — отсчёт у позиции
    /// от цены ЗАКРЫТИЯ стороны, и проверка обязана это различать.
    pub(crate) async fn quote(st: &AppState, bid: i64, ask: i64, last: i64) {
        st.tickers.lock().await.insert(
            1,
            feed::Ticker {
                last,
                change: 0.0,
                high: last,
                bid,
                ask,
                ..Default::default()
            },
        );
    }

    pub(crate) const PX: i64 = 5_000_000;

    pub(crate) fn on(mut st: AppState) -> AppState {
        st.sltp_edit_enabled = true;
        st
    }

    pub(crate) async fn open(app: &Router, side: &str) -> u64 {
        let (s, d) = send(
            app,
            "POST",
            "/deals",
            Some(json!({"instrument": 1, "side": side, "qty": 1000})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{d}");
        d["id"].as_u64().unwrap()
    }

    pub(crate) async fn kinds(st: &AppState) -> Vec<String> {
        st.journals
            .lock()
            .await
            .get(&DEFAULT_USER)
            .map(|j| j.events().iter().map(|e| e.canon().to_string()).collect())
            .unwrap_or_default()
    }

    pub(crate) async fn deal(app: &Router, id: u64) -> Value {
        let (_, list) = send(app, "GET", "/deals", None).await;
        list.as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == id)
            .cloned()
            .unwrap_or(Value::Null)
    }

    pub(crate) async fn reconciled(app: &Router) {
        let (_, jb) = send(app, "GET", "/account/journal", None).await;
        assert_eq!(jb["status"], "reconciled", "{jb}");
    }

    fn code(r: &(StatusCode, Value)) -> (StatusCode, Option<&str>) {
        (r.0, r.1["error"].as_str())
    }

    // ---- Флаг ---------------------------------------------------------------

    #[tokio::test]
    async fn with_the_flag_off_the_edit_is_refused_and_nothing_is_written() {
        let st = build_state();
        quote(&st, PX, PX, PX).await;
        let app = router(st.clone());
        let id = open(&app, "buy").await;
        let before = kinds(&st).await;

        let (_, acct) = send(&app, "GET", "/account", None).await;
        assert_eq!(acct["sltp_edit"], false, "фронт узнаёт флаг из /account");

        let r = send(
            &app,
            "PUT",
            &format!("/deals/{id}/levels"),
            Some(json!({"sl": PX - 10_000})),
        )
        .await;
        assert_eq!(code(&r), (StatusCode::CONFLICT, Some("sltp_edit_disabled")));
        let r = send(&app, "PUT", "/pending/1/levels", Some(json!({"sl": null}))).await;
        assert_eq!(code(&r), (StatusCode::CONFLICT, Some("sltp_edit_disabled")));
        assert_eq!(kinds(&st).await, before, "ни одной записи");
        assert_eq!(deal(&app, id).await["sl"], Value::Null);
    }

    #[tokio::test]
    async fn with_the_flag_on_account_says_so() {
        let st = on(build_state());
        let app = router(st);
        let (_, acct) = send(&app, "GET", "/account", None).await;
        assert_eq!(acct["sltp_edit"], true);
    }

    // ---- Позиция: поставить, сдвинуть, снять --------------------------------

    #[tokio::test]
    async fn set_move_and_remove_on_an_open_position() {
        let st = on(build_state());
        quote(&st, PX, PX, PX).await;
        let app = router(st.clone());
        let id = open(&app, "buy").await;
        let uri = format!("/deals/{id}/levels");

        let (s, b) = send(
            &app,
            "PUT",
            &uri,
            Some(json!({"sl": PX - 50_000, "tp": PX + 80_000})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{b}");
        assert_eq!(
            b,
            json!({"id": id, "sl": PX - 50_000, "tp": PX + 80_000, "changed": true})
        );
        assert_eq!(deal(&app, id).await["sl"], PX - 50_000);

        // Сдвинуть только стоп: тейк остаётся.
        let (s, b) = send(&app, "PUT", &uri, Some(json!({"sl": PX - 20_000}))).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        assert_eq!(
            (b["sl"].as_i64(), b["tp"].as_i64()),
            (Some(PX - 20_000), Some(PX + 80_000))
        );

        // Повтор — 200 без записи.
        let n = kinds(&st).await.len();
        let (s, b) = send(&app, "PUT", &uri, Some(json!({"sl": PX - 20_000}))).await;
        assert_eq!((s, b["changed"].as_bool()), (StatusCode::OK, Some(false)));
        assert_eq!(kinds(&st).await.len(), n, "повтор не пишет");

        // Снять тейк.
        let (s, b) = send(&app, "PUT", &uri, Some(json!({"tp": null}))).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        assert_eq!(deal(&app, id).await["tp"], Value::Null);

        let ks = kinds(&st).await;
        let levels: Vec<String> = ks
            .iter()
            .filter(|k| k.starts_with("position_levels_changed"))
            .cloned()
            .collect();
        assert_eq!(
            levels,
            [
                format!(
                    "position_levels_changed id={id} sl={} tp={}",
                    PX - 50_000,
                    PX + 80_000
                ),
                format!(
                    "position_levels_changed id={id} sl={} tp={}",
                    PX - 20_000,
                    PX + 80_000
                ),
                format!("position_levels_changed id={id} sl={} tp=-", PX - 20_000),
            ]
        );
        let last = st.journals.lock().await[&DEFAULT_USER]
            .events()
            .last()
            .unwrap()
            .origin()
            .clone();
        assert_eq!(last, Origin::Client, "правку сделал человек");
        reconciled(&app).await;
    }

    #[tokio::test]
    async fn bad_bodies_are_refused_by_name() {
        let st = on(build_state());
        quote(&st, PX, PX, PX).await;
        let app = router(st.clone());
        let id = open(&app, "buy").await;
        let uri = format!("/deals/{id}/levels");
        for (body, c) in [
            (json!({}), "levels_required"),
            (json!({"sl": 0}), "invalid_level"),
            (json!({"tp": -5}), "invalid_level"),
            (json!({"sl": 1.5}), "bad_request"),
        ] {
            let r = send(&app, "PUT", &uri, Some(body.clone())).await;
            assert_eq!(code(&r), (StatusCode::BAD_REQUEST, Some(c)), "{body}");
        }
    }

    // ---- Проверки: минимум площадки и «уже пробит» --------------------------

    #[tokio::test]
    async fn the_minimum_is_measured_from_the_closing_quote() {
        let mut st = on(build_state());
        st.stops = Arc::new(stops::StopSettings::new(50)); // 0.5 %
                                                           // bid 49 900.00, ask 50 100.00, last 50 000.00.
        quote(&st, PX - 10_000, PX + 10_000, PX).await;
        let app = router(st.clone());
        let long = open(&app, "buy").await;
        let short = open(&app, "sell").await;

        // Покупку закрывают по bid: 0.5 % от 49 900.00 = 249.50 → SL ≤ 49 650.50.
        let (s, b) = send(
            &app,
            "PUT",
            &format!("/deals/{long}/levels"),
            Some(json!({"sl": PX - 10_000 - 24_949})),
        )
        .await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{b}");
        assert_eq!(b["error"], "stop_loss_too_close");
        assert_eq!(b["reference"], PX - 10_000, "от bid");
        assert_eq!(b["min_distance"], 24_950);
        assert_eq!(b["limit"], PX - 10_000 - 24_950);
        assert_eq!(b["min_stop_bps"], 50);
        let (s, _) = send(
            &app,
            "PUT",
            &format!("/deals/{long}/levels"),
            Some(json!({"sl": PX - 10_000 - 24_950})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "ровно на границе — можно");

        // Продажу закрывают по ask: TP ≤ ask − d.
        let (s, b) = send(
            &app,
            "PUT",
            &format!("/deals/{short}/levels"),
            Some(json!({"tp": PX})),
        )
        .await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{b}");
        assert_eq!(b["error"], "take_profit_too_close");
        assert_eq!(b["reference"], PX + 10_000, "от ask");
        // Снять — всегда можно: проверять нечего.
        let (s, _) = send(
            &app,
            "PUT",
            &format!("/deals/{long}/levels"),
            Some(json!({"sl": null})),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        reconciled(&app).await;
    }

    #[tokio::test]
    async fn a_level_already_crossed_is_refused_in_plain_words() {
        let st = on(build_state());
        quote(&st, PX, PX + 100, PX).await;
        let app = router(st.clone());
        let id = open(&app, "buy").await;
        let before = kinds(&st).await;
        let r = send(
            &app,
            "PUT",
            &format!("/deals/{id}/levels"),
            Some(json!({"sl": PX + 50})),
        )
        .await;
        assert_eq!(
            code(&r),
            (StatusCode::CONFLICT, Some("stop_loss_already_reached"))
        );
        let r = send(
            &app,
            "PUT",
            &format!("/deals/{id}/levels"),
            Some(json!({"tp": PX - 1})),
        )
        .await;
        assert_eq!(
            code(&r),
            (StatusCode::CONFLICT, Some("take_profit_already_reached"))
        );
        assert_eq!(kinds(&st).await, before);
    }

    // ---- Ворота счёта -------------------------------------------------------

    #[tokio::test]
    async fn frozen_quarantined_and_foreign_are_refused() {
        let st = on(build_state());
        quote(&st, PX, PX, PX).await;
        let app = router(st.clone());
        let id = open(&app, "buy").await;
        let uri = format!("/deals/{id}/levels");

        // Чужая или несуществующая позиция.
        let r = send(&app, "PUT", "/deals/999/levels", Some(json!({"sl": 1}))).await;
        assert_eq!(code(&r), (StatusCode::NOT_FOUND, Some("unknown_position")));
        let r = send(&app, "PUT", "/pending/999/levels", Some(json!({"sl": 1}))).await;
        assert_eq!(code(&r), (StatusCode::NOT_FOUND, Some("unknown_pending")));

        // Заморозка оператором — запись в ленте, как в бою.
        {
            let mut js = st.journals.lock().await;
            let j = js.get_mut(&DEFAULT_USER).unwrap();
            let op = broker::journal::Operator::new("ops", "проверка", "INC-1").unwrap();
            for e in j.seal(
                1,
                &Origin::Operator(op),
                &[EventKind::TradingFreezeSet { frozen: true }],
            ) {
                j.commit(e).unwrap();
            }
        }
        let r = send(&app, "PUT", &uri, Some(json!({"sl": PX - 10_000}))).await;
        assert_eq!(code(&r), (StatusCode::FORBIDDEN, Some("account_frozen")));

        st.quarantined.lock().await.insert(DEFAULT_USER);
        let r = send(&app, "PUT", &uri, Some(json!({"sl": PX - 10_000}))).await;
        assert_eq!(code(&r), (StatusCode::LOCKED, Some("account_quarantined")));
        assert_eq!(deal(&app, id).await["sl"], Value::Null);
    }

    #[tokio::test]
    async fn editing_without_an_account_does_not_open_one() {
        let st = on(build_state());
        let app = router(st.clone());
        let (s, _) = send(&app, "PUT", "/deals/1/levels", Some(json!({"sl": 1}))).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (_, acct) = send(&app, "GET", "/account", None).await;
        assert_eq!(acct["opened"], false, "правка не заводит счёт");
        assert!(kinds(&st).await.is_empty());
    }

    // ---- Отложенный ордер ---------------------------------------------------

    #[tokio::test]
    async fn pending_levels_measure_from_the_order_price_and_ride_into_the_fill() {
        let mut st = on(build_state());
        st.stops = Arc::new(stops::StopSettings::new(100)); // 1 %
        quote(&st, PX, PX, PX).await;
        let app = router(st.clone());
        let price = PX - 100_000; // лимит на покупку 49 000.00
        let (s, o) = send(
            &app,
            "POST",
            "/pending",
            Some(json!({"instrument": 1, "side": "buy", "qty": 1000, "price": price})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{o}");
        let id = o["id"].as_u64().unwrap();
        let uri = format!("/pending/{id}/levels");

        // 1 % от цены ордера (не от рынка) = 490.00.
        let (s, b) = send(&app, "PUT", &uri, Some(json!({"sl": price - 48_999}))).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{b}");
        assert_eq!(
            (b["reference"].as_i64(), b["limit"].as_i64()),
            (Some(price), Some(price - 49_000))
        );
        let (s, b) = send(&app, "PUT", &uri, Some(json!({"sl": price + 1}))).await;
        assert_eq!(
            s,
            StatusCode::UNPROCESSABLE_ENTITY,
            "выше цены покупки — тоже слишком близко: {b}"
        );

        let (s, b) = send(
            &app,
            "PUT",
            &uri,
            Some(json!({"sl": price - 49_000, "tp": price + 60_000})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let (_, list) = send(&app, "GET", "/pending", None).await;
        assert_eq!(list[0]["sl"], price - 49_000);
        assert!(kinds(&st).await.contains(&format!(
            "pending_levels_changed id={id} sl={} tp={}",
            price - 49_000,
            price + 60_000
        )));

        // Рынок дошёл до ордера — позиция рождается с новыми уровнями.
        quote(&st, price, price, price).await;
        monitor_tick(&st).await;
        let (_, deals) = send(&app, "GET", "/deals", None).await;
        assert_eq!(deals[0]["sl"], price - 49_000);
        assert_eq!(deals[0]["tp"], price + 60_000);
        reconciled(&app).await;
    }

    // ---- Движок стопов и гонка с закрытием ----------------------------------

    #[tokio::test]
    async fn the_monitor_fires_on_the_new_level_immediately() {
        let st = on(build_state());
        quote(&st, PX, PX, PX).await;
        let app = router(st.clone());
        let id = open(&app, "buy").await;
        let (s, _) = send(
            &app,
            "PUT",
            &format!("/deals/{id}/levels"),
            Some(json!({"sl": PX - 1_000})),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        quote(&st, PX - 2_000, PX - 2_000, PX - 2_000).await;
        monitor_tick(&st).await;
        let (_, closed) = send(&app, "GET", "/deals/closed", None).await;
        assert_eq!(closed[0]["level"], PX - 1_000, "закрыта по новому уровню");
        let ks = kinds(&st).await;
        assert!(ks.last().unwrap().contains("cause=stop_loss"), "{ks:?}");
        // Правка уже закрытой позиции — «нет такой», а не правка.
        let r = send(
            &app,
            "PUT",
            &format!("/deals/{id}/levels"),
            Some(json!({"sl": PX - 5_000})),
        )
        .await;
        assert_eq!(code(&r), (StatusCode::NOT_FOUND, Some("unknown_position")));
        reconciled(&app).await;
    }

    /// Правки и прогон триггеров наперегонки: каждая правка либо применилась
    /// до закрытия, либо получила отказ — и лента в конце сходится, счёт не в
    /// карантине, правки после закрытия в ленте нет.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn edits_racing_a_stop_out_leave_a_consistent_journal() {
        for round in 0..20 {
            let st = on(build_state());
            quote(&st, PX, PX, PX).await;
            let app = router(st.clone());
            let id = open(&app, "buy").await;
            let (s, _) = send(
                &app,
                "PUT",
                &format!("/deals/{id}/levels"),
                Some(json!({"sl": PX - 1_000})),
            )
            .await;
            assert_eq!(s, StatusCode::OK);

            let mut tasks = Vec::new();
            for k in 0..8i64 {
                let app = app.clone();
                tasks.push(tokio::spawn(async move {
                    send(
                        &app,
                        "PUT",
                        &format!("/deals/{id}/levels"),
                        Some(json!({"sl": PX - 1_000 - k * 10})),
                    )
                    .await
                    .0
                }));
            }
            let st2 = st.clone();
            let monitor = tokio::spawn(async move {
                quote(&st2, PX - 5_000, PX - 5_000, PX - 5_000).await;
                monitor_tick(&st2).await;
            });
            for t in tasks {
                let s = t.await.unwrap();
                // OK — успела до закрытия; 404 — позиция уже закрыта;
                // 409 — цена уже ушла за уровень (уровень «пробит»).
                assert!(
                    [StatusCode::OK, StatusCode::NOT_FOUND, StatusCode::CONFLICT].contains(&s),
                    "раунд {round}: {s}"
                );
            }
            monitor.await.unwrap();
            monitor_tick(&st).await;

            assert!(st.quarantined.lock().await.is_empty(), "раунд {round}");
            let (_, deals) = send(&app, "GET", "/deals", None).await;
            assert_eq!(
                deals.as_array().unwrap().len(),
                0,
                "раунд {round}: позиция закрыта стопом"
            );
            reconciled(&app).await;
            let ks = kinds(&st).await;
            let closed_at = ks
                .iter()
                .position(|k| k.starts_with("position_closed"))
                .unwrap();
            assert!(
                ks[closed_at + 1..]
                    .iter()
                    .all(|k| !k.starts_with("position_levels_changed")),
                "раунд {round}: правка после закрытия не записана: {ks:?}"
            );
        }
    }

    /// Та же гонка у закрытия по команде: она существовала до правки SL/TP
    /// (17.11) и чинится тем же местом — операция не сливает чужую очередь.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_close_racing_a_stop_out_does_not_quarantine_the_account() {
        for round in 0..20 {
            let st = on(build_state());
            quote(&st, PX, PX, PX).await;
            let app = router(st.clone());
            let id = open(&app, "buy").await;
            send(
                &app,
                "PUT",
                &format!("/deals/{id}/levels"),
                Some(json!({"sl": PX - 1_000})),
            )
            .await;
            let a2 = app.clone();
            let close = tokio::spawn(async move {
                send(&a2, "POST", &format!("/deals/{id}/close"), None)
                    .await
                    .0
            });
            let st2 = st.clone();
            let monitor = tokio::spawn(async move {
                quote(&st2, PX - 5_000, PX - 5_000, PX - 5_000).await;
                monitor_tick(&st2).await;
            });
            let s = close.await.unwrap();
            assert!(
                [StatusCode::OK, StatusCode::NOT_FOUND].contains(&s),
                "раунд {round}: {s}"
            );
            monitor.await.unwrap();
            monitor_tick(&st).await;
            assert!(st.quarantined.lock().await.is_empty(), "раунд {round}");
            reconciled(&app).await;
            let closes = kinds(&st)
                .await
                .iter()
                .filter(|k| k.starts_with("position_closed"))
                .count();
            assert_eq!(closes, 1, "раунд {round}: позиция закрыта ровно один раз");
        }
    }

    // ---- Откат: лента с записью нового вида читается при выключенном флаге --

    /// **Доказательство условия отката.** Шаг 2 (флаг 1) пишет записи нового
    /// вида; откат после этого — на ЭТУ сборку с флагом 0. Она обязана
    /// поднять такую ленту без карантина и с теми же уровнями, а движок —
    /// работать по ним. Запись при этом выключена.
    #[tokio::test]
    async fn a_journal_with_level_records_restores_with_the_flag_off() {
        let store = Arc::new(persistence::MemoryStore::new());
        rollback_scenario(store).await;
    }

    /// Сценарий отката — общий для хранилища в памяти и живого PostgreSQL.
    pub(crate) async fn rollback_scenario(store: Arc<dyn persistence::Persistence>) {
        let (deal_id, order_id) = {
            let st = on(build_state_with(store.clone()));
            restore_state(&st).await.expect("подъём пустого");
            quote(&st, PX, PX, PX).await;
            let app = router(st.clone());
            let id = open(&app, "buy").await;
            let (s, _) = send(
                &app,
                "PUT",
                &format!("/deals/{id}/levels"),
                Some(json!({"sl": PX - 3_000, "tp": PX + 9_000})),
            )
            .await;
            assert_eq!(s, StatusCode::OK);
            let (_, o) = send(
                &app,
                "POST",
                "/pending",
                Some(json!({"instrument": 1, "side": "sell", "qty": 1000, "price": PX + 50_000})),
            )
            .await;
            let oid = o["id"].as_u64().unwrap();
            let (s, _) = send(
                &app,
                "PUT",
                &format!("/pending/{oid}/levels"),
                Some(json!({"sl": PX + 60_000})),
            )
            .await;
            assert_eq!(s, StatusCode::OK);
            (id, oid)
        };

        // Перезапуск: то же хранилище, ФЛАГ ВЫКЛЮЧЕН.
        let st = build_state_with(store.clone());
        assert!(!st.sltp_edit_enabled, "предпосылка: флаг 0");
        restore_state(&st).await.expect("подъём обязан пройти");
        assert!(
            !st.quarantined.lock().await.contains(&DEFAULT_USER),
            "лента с записями нового вида — не повод для карантина"
        );
        let ks = kinds(&st).await;
        assert!(
            ks.iter().any(|k| k.starts_with("position_levels_changed")),
            "{ks:?}"
        );
        assert!(
            ks.iter().any(|k| k.starts_with("pending_levels_changed")),
            "{ks:?}"
        );
        let app = router(st.clone());
        reconciled(&app).await;
        let d = deal(&app, deal_id).await;
        assert_eq!(
            (d["sl"].as_i64(), d["tp"].as_i64()),
            (Some(PX - 3_000), Some(PX + 9_000))
        );
        let (_, list) = send(&app, "GET", "/pending", None).await;
        assert_eq!(list[0]["id"], order_id);
        assert_eq!(list[0]["sl"], PX + 60_000);

        // Запись выключена, торговля — нет.
        let r = send(
            &app,
            "PUT",
            &format!("/deals/{deal_id}/levels"),
            Some(json!({"sl": null})),
        )
        .await;
        assert_eq!(code(&r), (StatusCode::CONFLICT, Some("sltp_edit_disabled")));
        // Движок стопов работает по поднятым уровням.
        quote(&st, PX - 3_000, PX - 3_000, PX - 3_000).await;
        monitor_tick(&st).await;
        let (_, closed) = send(&app, "GET", "/deals/closed", None).await;
        assert_eq!(closed[0]["level"], PX - 3_000);
        reconciled(&app).await;
        assert!(!st.quarantined.lock().await.contains(&DEFAULT_USER));

        // И третий подъём — после закрытия по поднятому уровню.
        let st3 = build_state_with(store);
        restore_state(&st3).await.expect("подъём");
        assert!(!st3.quarantined.lock().await.contains(&DEFAULT_USER));
    }

    /// То же на живом PostgreSQL: запись нового вида проходит через настоящий
    /// SQL (канон, отпечаток, триггер неизменяемости) и поднимается сборкой
    /// с выключенным флагом без карантина.
    #[tokio::test]
    #[ignore = "нужна живая БД: TEST_DATABASE_URL=postgres://... cargo test -p gateway -- --ignored"]
    async fn postgres_journal_with_level_records_restores_with_the_flag_off() {
        let url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL не задан — тест запущен без базы, которую он проверяет");
        let store = persistence::PgStore::connect(&url)
            .await
            .expect("подключение");
        persistence::Persistence::init(&store).await.expect("DDL");
        // Счёт автономного терминала один (`DEFAULT_USER`): чистим его строки,
        // иначе подъём прочитал бы чужую историю прошлого прогона.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE account_events DISABLE TRIGGER account_events_no_update")
            .execute(&pool)
            .await
            .unwrap();
        for t in [
            "positions",
            "pendings",
            "closed_deals",
            "accounts",
            "account_events",
        ] {
            sqlx::query(&format!("DELETE FROM {t} WHERE user_id = $1"))
                .bind(DEFAULT_USER.0 as i64)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("ALTER TABLE account_events ENABLE TRIGGER account_events_no_update")
            .execute(&pool)
            .await
            .unwrap();
        rollback_scenario(Arc::new(store)).await;
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM account_events WHERE user_id = $1 AND kind LIKE '%levels_changed%'",
        )
        .bind(DEFAULT_USER.0 as i64)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 2, "обе записи нового вида лежат в базе");
    }
}
