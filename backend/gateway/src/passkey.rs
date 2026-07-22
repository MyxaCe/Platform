//! Passkey / WebAuthn (ADR-020).
//!
//! Вход без пароля: ключ хранится в устройстве (Face ID / отпечаток / PIN), сервер
//! держит только **публичный** ключ и проверяет подпись. Криптографию делает
//! `webauthn-rs`; здесь — состояние церемоний, хранение ключей и эндпоинты.
//!
//! - **Регистрация** (`/auth/passkey/register/*`) — под сессией: залогиненный
//!   пользователь привязывает ключ к своему аккаунту.
//! - **Вход** (`/auth/passkey/login/*`) — discoverable: браузер сам предлагает
//!   доступные ключи, сервер по ключу узнаёт, чей он, и открывает сессию.
//!
//! Состояние церемонии (challenge) живёт на сервере между start и finish, привязано
//! к одноразовому `flow`-id — так challenge нельзя подменить на стороне клиента.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use webauthn_rs::prelude::*;

use domain::account::UserId;

use super::{authed, err, now_unix, open_session, AppState, ApiErr, MeResp};

/// Собрать WebAuthn. RP ID — регистрируемый домен (`meriom.com`), поэтому ключ
/// действует на всех поддоменах (accounts/trade/my). Origin и id — из окружения,
/// чтобы на проде не править код.
pub(crate) fn build_webauthn() -> Arc<Webauthn> {
    let rp_id = std::env::var("WEBAUTHN_RP_ID").unwrap_or_else(|_| "meriom.com".to_string());
    let origin = std::env::var("WEBAUTHN_ORIGIN").unwrap_or_else(|_| "https://meriom.com".to_string());
    let url = Url::parse(&origin).expect("WEBAUTHN_ORIGIN — некорректный URL");
    let wa = WebauthnBuilder::new(&rp_id, &url)
        .expect("WEBAUTHN_RP_ID не является суффиксом origin")
        .allow_subdomains(true)
        .rp_name("ExchangePro")
        .build()
        .expect("не удалось собрать Webauthn");
    Arc::new(wa)
}

/// Незавершённая церемония, ждущая шага finish. Для входа храним и пользователя:
/// его определяем по почте на шаге start, а не по ключу (webauthn-rs здесь создаёт
/// не-discoverable ключи, поэтому вход идёт с известной почтой и allowCredentials).
pub(crate) enum PkFlow {
    Reg(UserId, PasskeyRegistration),
    Auth(UserId, PasskeyAuthentication),
}

/// UserId ↔ Uuid: webauthn-rs идентифицирует пользователя по Uuid, а у нас id — u64.
/// Отображение детерминированное и обратимое, отдельное хранилище не нужно.
fn uuid_of(u: UserId) -> Uuid {
    Uuid::from_u128(u.0 as u128)
}

/// Ключ таблицы passkeys — id учётных данных ключа в url-safe base64.
pub(crate) fn cred_key(pk: &Passkey) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(pk.cred_id())
}

fn new_flow_id() -> String {
    // Переиспользуем генератор случайных токенов — нужен просто непредсказуемый id.
    auth::new_session_token().0
}

async fn put_flow(st: &AppState, flow: String, f: PkFlow) {
    st.pk_flows.lock().await.insert(flow, (f, now_unix() + 300)); // 5 минут на завершение
}
async fn take_flow(st: &AppState, flow: &str) -> Option<PkFlow> {
    let mut m = st.pk_flows.lock().await;
    let now = now_unix();
    m.retain(|_, (_, exp)| *exp > now); // попутно чистим протухшие
    m.remove(flow).map(|(f, _)| f)
}

// ---- Регистрация ключа (под сессией) --------------------------------------

pub(crate) async fn register_start(State(st): State<AppState>, headers: HeaderMap) -> Result<Json<serde_json::Value>, ApiErr> {
    let user = authed(&st, &headers).await?;
    let email = st.users.lock().await.email_of(user).unwrap_or_default();
    // Уже привязанные ключи исключаем, чтобы не регистрировать один дважды.
    let exclude: Vec<CredentialID> = st.users.lock().await.passkeys_of(user).iter().map(|p| p.cred_id().clone()).collect();

    let (ccr, reg) = st
        .webauthn
        .start_passkey_registration(uuid_of(user), &email, &email, Some(exclude))
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "webauthn"))?;

    let flow = new_flow_id();
    put_flow(&st, flow.clone(), PkFlow::Reg(user, reg)).await;
    Ok(Json(serde_json::json!({ "flow": flow, "options": ccr })))
}

#[derive(Deserialize)]
pub(crate) struct RegisterFinish {
    flow: String,
    credential: RegisterPublicKeyCredential,
}

pub(crate) async fn register_finish(State(st): State<AppState>, headers: HeaderMap, Json(body): Json<RegisterFinish>) -> Result<Json<serde_json::Value>, ApiErr> {
    let user = authed(&st, &headers).await?;
    let flow = take_flow(&st, &body.flow).await.ok_or_else(|| err(StatusCode::BAD_REQUEST, "unknown or expired flow"))?;
    let PkFlow::Reg(flow_user, reg) = flow else {
        return Err(err(StatusCode::BAD_REQUEST, "wrong flow type"));
    };
    // Ключ привязывается к тому, кто начал церемонию, а не к чужой сессии.
    if flow_user != user {
        return Err(err(StatusCode::FORBIDDEN, "flow belongs to another user"));
    }
    let passkey = st
        .webauthn
        .finish_passkey_registration(&body.credential, &reg)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "registration failed"))?;

    let key = cred_key(&passkey);
    let data = serde_json::to_string(&passkey).map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "serialize"))?;
    st.users.lock().await.add_passkey(user, passkey);
    if let Err(e) = st.store.save_passkey(user, &key, &data).await {
        eprintln!("[passkey] ключ не сохранён: {e}");
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---- Вход по ключу (почта известна с шага 1) ------------------------------

#[derive(Deserialize)]
pub(crate) struct LoginStart {
    email: String,
}

pub(crate) async fn login_start(State(st): State<AppState>, Json(body): Json<LoginStart>) -> Result<Json<serde_json::Value>, ApiErr> {
    let email = auth::normalize_email(&body.email);
    let user = st.users.lock().await.find_by_email(&email).map(|(id, _)| id);
    let keys = match user {
        Some(u) => st.users.lock().await.passkeys_of(u),
        None => Vec::new(),
    };
    // Одинаковый ответ, когда ключей нет: и «нет такого пользователя», и «у него нет
    // ключей» отдают 404 — не раскрываем, зарегистрирована ли почта здесь тоже.
    if keys.is_empty() {
        return Err(err(StatusCode::NOT_FOUND, "no passkey for this account"));
    }
    let user = user.unwrap();
    let (rcr, authst) = st
        .webauthn
        .start_passkey_authentication(&keys)
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "webauthn"))?;
    let flow = new_flow_id();
    put_flow(&st, flow.clone(), PkFlow::Auth(user, authst)).await;
    Ok(Json(serde_json::json!({ "flow": flow, "options": rcr })))
}

#[derive(Deserialize)]
pub(crate) struct LoginFinish {
    flow: String,
    credential: PublicKeyCredential,
}

pub(crate) async fn login_finish(State(st): State<AppState>, Json(body): Json<LoginFinish>) -> Result<impl IntoResponse, ApiErr> {
    let flow = take_flow(&st, &body.flow).await.ok_or_else(|| err(StatusCode::BAD_REQUEST, "unknown or expired flow"))?;
    let PkFlow::Auth(user, authst) = flow else {
        return Err(err(StatusCode::BAD_REQUEST, "wrong flow type"));
    };

    let res = st
        .webauthn
        .finish_passkey_authentication(&body.credential, &authst)
        .map_err(|_| err(StatusCode::UNAUTHORIZED, "authentication failed"))?;

    // Счётчик подписей мог вырасти — обновляем и пересохраняем затронутый ключ.
    let mut keys = st.users.lock().await.passkeys_of(user);
    for pk in keys.iter_mut() {
        if pk.update_credential(&res).is_some() {
            let key = cred_key(pk);
            if let Ok(data) = serde_json::to_string(pk) {
                let _ = st.store.save_passkey(user, &key, &data).await;
            }
        }
    }
    st.users.lock().await.set_passkeys(user, keys);

    let email = st.users.lock().await.email_of(user).unwrap_or_default();
    let verified = st.users.lock().await.is_verified(user);
    let ck = open_session(&st, user).await?;
    Ok(([(axum::http::header::SET_COOKIE, ck)], Json(MeResp { user_id: user.0, email, verified })))
}
