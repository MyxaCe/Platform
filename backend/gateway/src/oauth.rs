//! Вход через внешних провайдеров по OAuth 2.0 (ADR-021): Яндекс сейчас, Google и
//! прочие — тем же скелетом. Провайдер отдаёт нам подтверждённую почту, по ней мы
//! находим или создаём пользователя и открываем свою сессию.
//!
//! Поток: `/auth/oauth/:provider/start` ставит cookie со state (защита от CSRF) и
//! редиректит к провайдеру → тот возвращает на `/callback?code&state` → сверяем
//! state, меняем code на токен, получаем почту, логиним, редиректим в кабинет.
//!
//! Секреты (`*_CLIENT_ID`/`*_CLIENT_SECRET`) — только из окружения, в коде и git их нет.

use axum::extract::{Path, Query, State};
use axum::http::header::{LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use super::{open_session, AppState};

/// Как передаётся токен в запрос userinfo: у Яндекса — `OAuth`, у Google — `Bearer`.
#[derive(Clone, Copy)]
enum TokenScheme {
    OAuth,
    Bearer,
}

struct Provider {
    authorize: &'static str,
    token: &'static str,
    userinfo: &'static str,
    scope: &'static str,
    /// Поле с почтой в ответе userinfo.
    email_field: &'static str,
    scheme: TokenScheme,
    client_id: String,
    client_secret: String,
}

/// Конфиг провайдера, если он настроен (заданы client_id и secret).
fn provider(name: &str) -> Option<Provider> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    match name {
        "yandex" => Some(Provider {
            authorize: "https://oauth.yandex.ru/authorize",
            token: "https://oauth.yandex.ru/token",
            userinfo: "https://login.yandex.ru/info?format=json",
            scope: "login:email login:info",
            email_field: "default_email",
            scheme: TokenScheme::OAuth,
            client_id: env("YANDEX_CLIENT_ID")?,
            client_secret: env("YANDEX_CLIENT_SECRET")?,
        }),
        "google" => Some(Provider {
            authorize: "https://accounts.google.com/o/oauth2/v2/auth",
            token: "https://oauth2.googleapis.com/token",
            userinfo: "https://openidconnect.googleapis.com/v1/userinfo",
            scope: "openid email profile",
            email_field: "email",
            scheme: TokenScheme::Bearer,
            client_id: env("GOOGLE_CLIENT_ID")?,
            client_secret: env("GOOGLE_CLIENT_SECRET")?,
        }),
        _ => None,
    }
}

/// Список настроенных провайдеров — интерфейс включает только их кнопки.
pub(crate) async fn providers() -> axum::Json<Vec<&'static str>> {
    axum::Json(["yandex", "google"].into_iter().filter(|p| provider(p).is_some()).collect())
}

/// URL-кодирование значения query-параметра (пробелы, `:` и т.п.).
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Redirect URI строится из хоста запроса, чтобы совпадать с зарегистрированным у
/// провайдера (`https://accounts.<домен>/auth/oauth/<provider>/callback`). Мы всегда
/// за HTTPS-edge, поэтому схема — https.
fn redirect_uri(headers: &HeaderMap, provider: &str) -> String {
    let host = headers.get(axum::http::header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("accounts.meriom.com");
    format!("https://{host}/auth/oauth/{provider}/callback")
}

/// Короткоживущая cookie со state: сверяется на callback (защита от CSRF).
fn state_cookie(value: &str, max_age: i64) -> String {
    let secure = if std::env::var("COOKIE_SECURE").is_ok() { "; Secure" } else { "" };
    format!("oauth_state={value}; HttpOnly; SameSite=Lax; Path=/auth/oauth; Max-Age={max_age}{secure}")
}

/// Увести обратно на страницу входа с пометкой об ошибке — вместо сырой 500.
fn back_to_login(headers: &HeaderMap) -> Response {
    let host = headers.get(axum::http::header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("accounts.meriom.com");
    Redirect::to(&format!("https://{host}/?error=oauth")).into_response()
}

pub(crate) async fn start(Path(name): Path<String>, State(_st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(p) = provider(&name) else {
        return back_to_login(&headers);
    };
    let state = auth::new_session_token().0;
    let url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}",
        p.authorize,
        enc(&p.client_id),
        enc(&redirect_uri(&headers, &name)),
        enc(p.scope),
        enc(&state),
    );
    let mut resp = Redirect::to(&url).into_response();
    if let Ok(c) = HeaderValue::from_str(&state_cookie(&state, 600)) {
        resp.headers_mut().append(SET_COOKIE, c);
    }
    resp
}

#[derive(Deserialize)]
pub(crate) struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

pub(crate) async fn callback(Path(name): Path<String>, State(st): State<AppState>, headers: HeaderMap, Query(q): Query<Callback>) -> Response {
    let Some(p) = provider(&name) else { return back_to_login(&headers) };
    // Пользователь отказал или провайдер вернул ошибку.
    if q.error.is_some() {
        return back_to_login(&headers);
    }
    // CSRF: state из query должен совпасть со state из нашей cookie.
    let cookie_state = super::cookie(&headers, "oauth_state");
    match (&q.state, &cookie_state) {
        (Some(a), Some(b)) if a == b => {}
        _ => return back_to_login(&headers),
    }
    let Some(code) = q.code else { return back_to_login(&headers) };

    let redirect = redirect_uri(&headers, &name);
    let client = reqwest::Client::new();

    // 1) code → access_token.
    let token_res = client
        .post(p.token)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("client_id", &p.client_id),
            ("client_secret", &p.client_secret),
            ("redirect_uri", &redirect),
        ])
        .send()
        .await;
    let access_token = match token_res {
        Ok(r) => match r.json::<serde_json::Value>().await {
            Ok(v) => v.get("access_token").and_then(|t| t.as_str()).map(String::from),
            Err(_) => None,
        },
        Err(_) => None,
    };
    let Some(access_token) = access_token else { return back_to_login(&headers) };

    // 2) access_token → почта.
    let auth_header = match p.scheme {
        TokenScheme::OAuth => format!("OAuth {access_token}"),
        TokenScheme::Bearer => format!("Bearer {access_token}"),
    };
    let info = client.get(p.userinfo).header(axum::http::header::AUTHORIZATION, auth_header).send().await;
    let email = match info {
        Ok(r) => match r.json::<serde_json::Value>().await {
            Ok(v) => v.get(p.email_field).and_then(|e| e.as_str()).map(auth::normalize_email),
            Err(_) => None,
        },
        Err(_) => None,
    };
    let Some(email) = email.filter(|e| auth::is_email_like(e)) else { return back_to_login(&headers) };

    // 3) Найти или создать пользователя по подтверждённой почте.
    let (user, created) = {
        let mut reg = st.users.lock().await;
        match reg.find_by_email(&email) {
            Some((id, _)) => (id, false),
            None => (reg.create_with_password(email.clone(), String::new()), true),
        }
    };
    if created {
        // OAuth-аккаунт без пароля: хэш пустой, войти паролем нельзя, пока не задан.
        if let Err(e) = st.store.save_credentials(user, &email, "").await {
            eprintln!("[oauth] учётные данные не сохранены: {e}");
        }
    }
    // Почта из OAuth подтверждена провайдером — шаг с кодом не нужен.
    st.users.lock().await.mark_verified(user);

    // 4) Наша сессия + возврат в кабинет. Заодно гасим cookie со state.
    let session_ck = match open_session(&st, user).await {
        Ok(c) => c,
        Err(_) => return back_to_login(&headers),
    };
    let host = headers.get(axum::http::header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("accounts.meriom.com");
    let base = host.strip_prefix("accounts.").unwrap_or(host);
    let mut resp = Response::new(axum::body::Body::empty());
    *resp.status_mut() = StatusCode::SEE_OTHER;
    resp.headers_mut().insert(LOCATION, HeaderValue::from_str(&format!("https://my.{base}/")).unwrap());
    if let Ok(c) = HeaderValue::from_str(&session_ck) {
        resp.headers_mut().append(SET_COOKIE, c);
    }
    if let Ok(c) = HeaderValue::from_str(&state_cookie("", 0)) {
        resp.headers_mut().append(SET_COOKIE, c);
    }
    resp
}
