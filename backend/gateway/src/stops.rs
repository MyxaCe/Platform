//! # stops — минимальное расстояние стоп-лосса и тейк-профита от цены
//!
//! Настройка **площадки** (сайта), задаётся из его админки:
//!
//! * `PUT /internal/sites/{tenant}/settings/stops` `{"min_stop_bps": N}` → `200 {"min_stop_bps": N}`
//! * `GET /internal/sites/{tenant}/settings/stops` → `200 {"min_stop_bps": N}`
//!
//! `N` — целое `0..=10000`, базисные пункты от текущей цены (10 bps = 0.1 %).
//! `0` — ограничения нет. Авторизация — та же, что у денежных ручек
//! (`X-Internal-Token` = `INTERNAL_MONEY_TOKEN`): нет заголовка — `401`,
//! чужой — `403`, секрет не задан — ручек нет (`404`).
//!
//! До первой записи площадки действует `MIN_STOP_BPS` окружения (или `0`).
//!
//! ## Почему не запись журнала счёта
//!
//! Минимум — свойство площадки, а не событие счёта, и хранится отдельной
//! строкой настроек (`site_settings`). Это ещё и условие отката: новый вид
//! записи в журнале счёта прежний образ не разберёт и отправит счёт в
//! карантин, а незнакомую таблицу он просто не прочитает.
//!
//! ## Где проверяется
//!
//! На сервере, при открытии позиции (`POST /deals`) и размещении лимитного
//! ордера (`POST /pending`) — это все пути, которыми SL/TP попадают в брокер.
//! Отсчёт — от **рыночной** цены стороны сделки (ask для покупки, bid для
//! продажи), у лимитного ордера — от цены ордера. Фронт берёт значение из
//! `/account` (`min_stop_bps`) и подставляет уровни сам, но граница — здесь.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use broker::PosSide;

use super::AppState;

/// Верхняя граница: 10 000 bps = 100 % цены.
pub const MAX_MIN_STOP_BPS: i64 = 10_000;

/// Минимум стопов по площадкам. Значение площадки, которой не назначали, —
/// `default_bps` из окружения.
pub struct StopSettings {
    default_bps: i64,
    by_tenant: Mutex<HashMap<String, i64>>,
}

impl StopSettings {
    pub fn new(default_bps: i64) -> Self {
        Self {
            default_bps,
            by_tenant: Mutex::new(HashMap::new()),
        }
    }

    /// Действующий минимум. Без площадки (автономный терминал) — умолчание.
    pub async fn for_tenant(&self, tenant: Option<&str>) -> i64 {
        match tenant {
            Some(t) => self
                .by_tenant
                .lock()
                .await
                .get(t)
                .copied()
                .unwrap_or(self.default_bps),
            None => self.default_bps,
        }
    }

    async fn set(&self, tenant: &str, bps: i64) {
        self.by_tenant.lock().await.insert(tenant.to_string(), bps);
    }

    /// Поднять назначенные площадками значения из хранилища (на старте).
    pub async fn load(&self, rows: Vec<(String, i64)>) {
        let mut m = self.by_tenant.lock().await;
        for (t, b) in rows {
            m.insert(t, b);
        }
    }
}

/// `MIN_STOP_BPS` из окружения. Не задано — `0`; неразборчивое или вне
/// `0..=10000` — тоже `0`, и громко: испорченная настройка не должна молча
/// закрыть торговлю со стопами.
pub(crate) fn default_from_env(raw: Option<&str>) -> i64 {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => 0,
        Some(s) => match s.parse::<i64>() {
            Ok(v) if (0..=MAX_MIN_STOP_BPS).contains(&v) => v,
            _ => {
                eprintln!(
                    "[stops] MIN_STOP_BPS=«{s}» не целое от 0 до {MAX_MIN_STOP_BPS} — взят 0 (без ограничения)"
                );
                0
            }
        },
    }
}

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(
        "/internal/sites/:tenant/settings/stops",
        get(get_stops).put(put_stops),
    )
}

#[derive(Serialize)]
struct StopsResp {
    min_stop_bps: i64,
}

/// Лишние поля (оператор, причина) не мешают: сайт вправе прислать больше.
#[derive(Deserialize)]
struct StopsReq {
    min_stop_bps: serde_json::Value,
}

fn fail(code: StatusCode, error: &str, detail: Option<String>) -> Response {
    let mut body = serde_json::json!({ "error": error });
    if let Some(d) = detail {
        body["detail"] = serde_json::Value::from(d);
    }
    (code, Json(body)).into_response()
}

fn tenant_of(raw: &str) -> Option<String> {
    let t = raw.trim();
    (!t.is_empty() && t.len() <= 200).then(|| t.to_string())
}

async fn get_stops(
    State(st): State<AppState>,
    Path(tenant): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = super::money::authorize(&st, &headers) {
        return *r;
    }
    let Some(tenant) = tenant_of(&tenant) else {
        return fail(StatusCode::BAD_REQUEST, "bad_tenant", None);
    };
    let n = st.stops.for_tenant(Some(&tenant)).await;
    (StatusCode::OK, Json(StopsResp { min_stop_bps: n })).into_response()
}

async fn put_stops(
    State(st): State<AppState>,
    Path(tenant): Path<String>,
    headers: HeaderMap,
    body: Result<Json<StopsReq>, JsonRejection>,
) -> Response {
    if let Err(r) = super::money::authorize(&st, &headers) {
        return *r;
    }
    let Some(tenant) = tenant_of(&tenant) else {
        return fail(StatusCode::BAD_REQUEST, "bad_tenant", None);
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
    // Целое, а не «число, которое округлим»: 12.5 bps — не то, что можно
    // молча превратить в 12 или 13.
    let bps = match req.min_stop_bps.as_i64() {
        Some(v) if (0..=MAX_MIN_STOP_BPS).contains(&v) => v,
        _ => {
            return fail(
                StatusCode::BAD_REQUEST,
                "invalid_min_stop_bps",
                Some(format!("min_stop_bps — целое от 0 до {MAX_MIN_STOP_BPS}")),
            )
        }
    };
    // Сначала хранилище, потом память: «записано» без записи обнаружилось бы
    // только после рестарта.
    if let Err(e) = st.store.save_site_min_stop(&tenant, bps).await {
        eprintln!("[stops] минимум стопов площадки {tenant} не записан: {e}");
        return fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "settings_store_unavailable",
            None,
        );
    }
    let before = st.stops.for_tenant(Some(&tenant)).await;
    st.stops.set(&tenant, bps).await;
    if before != bps {
        println!("[stops] площадка {tenant}: минимум стопов {before} → {bps} bps");
    }
    (StatusCode::OK, Json(StopsResp { min_stop_bps: bps })).into_response()
}

// ============================ Проверка ======================================

/// Минимальное расстояние в сырых единицах цены: `ceil(ref × bps / 10000)`.
///
/// Вверх, а не вниз: уровень ровно на этом расстоянии допустим, а округление
/// вниз пустило бы уровень ближе заявленного минимума.
pub fn min_distance(reference: i64, bps: i64) -> i64 {
    if bps <= 0 || reference <= 0 {
        return 0;
    }
    let num = i128::from(reference) * i128::from(bps);
    let d = (num + 9_999) / 10_000;
    i64::try_from(d).unwrap_or(i64::MAX)
}

/// Отказ: уровень ближе минимума. Несёт числа, по которым человек правит поле.
#[derive(Debug, PartialEq, Eq)]
pub struct TooClose {
    /// `stop_loss_too_close` | `take_profit_too_close`.
    pub code: &'static str,
    pub min_stop_bps: i64,
    /// Минимальное расстояние в сырых единицах цены.
    pub min_distance: i64,
    /// Ближайший допустимый уровень (сырые единицы).
    pub limit: i64,
    /// Цена, от которой отсчитано.
    pub reference: i64,
}

/// Проверить SL/TP против минимума. `None` у уровня — уровня нет, проверять нечего.
pub fn check(
    side: PosSide,
    reference: i64,
    sl: Option<i64>,
    tp: Option<i64>,
    bps: i64,
) -> Result<(), TooClose> {
    let d = min_distance(reference, bps);
    if d == 0 {
        return Ok(());
    }
    let long = matches!(side, PosSide::Long);
    let (sl_limit, tp_limit) = if long {
        (reference.saturating_sub(d), reference.saturating_add(d))
    } else {
        (reference.saturating_add(d), reference.saturating_sub(d))
    };
    let too_close = |code, limit| TooClose {
        code,
        min_stop_bps: bps,
        min_distance: d,
        limit,
        reference,
    };
    if let Some(sl) = sl {
        let ok = if long { sl <= sl_limit } else { sl >= sl_limit };
        if !ok {
            return Err(too_close("stop_loss_too_close", sl_limit));
        }
    }
    if let Some(tp) = tp {
        let ok = if long { tp >= tp_limit } else { tp <= tp_limit };
        if !ok {
            return Err(too_close("take_profit_too_close", tp_limit));
        }
    }
    Ok(())
}

/// Общее состояние модуля для `AppState`.
pub(crate) fn from_env() -> Arc<StopSettings> {
    let d = default_from_env(std::env::var("MIN_STOP_BPS").ok().as_deref());
    if d > 0 {
        println!("[stops] минимум стопов по умолчанию: {d} bps (MIN_STOP_BPS)");
    } else {
        println!("[stops] минимум стопов по умолчанию: нет (MIN_STOP_BPS не задан или 0)");
    }
    Arc::new(StopSettings::new(d))
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn distance_rounds_up_and_zero_means_none() {
        assert_eq!(min_distance(6_500_000, 50), 32_500);
        assert_eq!(min_distance(101, 1), 1, "0.0101 вверх до 1");
        assert_eq!(min_distance(6_500_000, 0), 0);
        assert_eq!(min_distance(0, 50), 0);
    }

    #[test]
    fn buy_and_sell_mirror_each_other() {
        let px = 10_000;
        // 100 bps от 10000 = 100.
        assert!(check(PosSide::Long, px, Some(9_900), Some(10_100), 100).is_ok());
        assert_eq!(
            check(PosSide::Long, px, Some(9_901), None, 100)
                .unwrap_err()
                .code,
            "stop_loss_too_close"
        );
        assert_eq!(
            check(PosSide::Long, px, None, Some(10_099), 100)
                .unwrap_err()
                .limit,
            10_100
        );
        assert!(check(PosSide::Short, px, Some(10_100), Some(9_900), 100).is_ok());
        let e = check(PosSide::Short, px, Some(10_050), None, 100).unwrap_err();
        assert_eq!((e.code, e.limit), ("stop_loss_too_close", 10_100));
        assert_eq!(
            check(PosSide::Short, px, None, Some(9_950), 100)
                .unwrap_err()
                .code,
            "take_profit_too_close"
        );
        // Без уровней и при нуле — проверять нечего.
        assert!(check(PosSide::Long, px, None, None, 100).is_ok());
        assert!(check(PosSide::Long, px, Some(9_999), Some(10_001), 0).is_ok());
    }

    #[test]
    fn env_default_is_zero_and_garbage_is_zero() {
        assert_eq!(default_from_env(None), 0);
        assert_eq!(default_from_env(Some(" 25 ")), 25);
        assert_eq!(default_from_env(Some("-1")), 0);
        assert_eq!(default_from_env(Some("10001")), 0);
        assert_eq!(default_from_env(Some("abc")), 0);
    }
}
