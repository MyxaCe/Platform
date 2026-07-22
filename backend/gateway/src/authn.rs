//! # authn — аутентификация (ADR-018/019/020/021)
//!
//! Весь HTTP-слой входа собран здесь: реестр пользователей и сессий, cookie,
//! парольный вход, коды подтверждения, ограничитель попыток. Соседние модули
//! [`super::passkey`] и [`super::oauth`] — быстрые входы поверх этого же реестра
//! и сессий. Чистая крипта (хэши, токены) — в отдельном крейте `auth`.
//!
//! Состояние (`users`, `codes`, `attempts`, `store`) живёт в [`super::AppState`];
//! обработчики берут его через `State`.

use std::collections::HashMap;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use domain::account::UserId;

use super::{err, mailer, ApiErr, AppState};

/// Реестр пользователей и сессий.
///
/// `by_token` — прежние dev-токены (alice-token/bob-token), живут до переезда терминала
/// на сессии (ADR-018). `by_email` — настоящие учётные записи: почта → (id, PHC-хэш).
/// `sessions` — хэш токена сессии → (чей, до какого времени в unix-секундах).
/// Память — рабочее состояние, БД — durable-проекция, как и у брокера (ADR-016).
#[derive(Debug, Default)]
pub struct UserRegistry {
    by_token: HashMap<String, UserId>,
    by_email: HashMap<String, (UserId, String)>,
    sessions: HashMap<String, (UserId, i64)>,
    /// Кто подтвердил почту кодом. Пока в памяти: до реальных денег переедет в БД
    /// вместе с остальным профилем.
    verified: std::collections::HashSet<UserId>,
    /// Публичные ключи Passkey по пользователям (ADR-020). Приватные ключи — только
    /// на устройстве клиента; у нас лежит то, чем проверяется подпись.
    passkeys: HashMap<UserId, Vec<webauthn_rs::prelude::Passkey>>,
    next: u64,
}
impl UserRegistry {
    pub fn create(&mut self, token: String) -> UserId {
        self.next += 1;
        let id = UserId(self.next);
        self.by_token.insert(token, id);
        id
    }
    pub fn resolve(&self, token: &str) -> Option<UserId> {
        self.by_token.get(token).copied()
    }
    /// Восстановить пользователя из хранилища на старте (ADR-016). `next` держим выше
    /// максимального загруженного id, иначе новый пользователь получил бы занятый id.
    pub fn restore(&mut self, token: String, id: UserId) {
        self.next = self.next.max(id.0);
        self.by_token.insert(token, id);
    }
    // ---- Учётные записи и сессии (ADR-018) --------------------------------

    /// Завести учётную запись. Почта уже нормализована вызывающим.
    pub fn create_with_password(&mut self, email: String, password_hash: String) -> UserId {
        self.next += 1;
        let id = UserId(self.next);
        self.by_email.insert(email, (id, password_hash));
        id
    }
    /// Поднять учётные данные из хранилища на старте.
    pub fn restore_credentials(&mut self, id: UserId, email: String, password_hash: String) {
        self.next = self.next.max(id.0);
        self.by_email.insert(email, (id, password_hash));
    }
    pub fn find_by_email(&self, email: &str) -> Option<(UserId, String)> {
        self.by_email.get(email).map(|(id, h)| (*id, h.clone()))
    }
    pub fn email_of(&self, user: UserId) -> Option<String> {
        self.by_email.iter().find(|(_, (id, _))| *id == user).map(|(e, _)| e.clone())
    }
    pub fn put_session(&mut self, token_hash: String, user: UserId, expires: i64) {
        self.sessions.insert(token_hash, (user, expires));
    }
    /// Сессия по хэшу токена; просроченная не считается действующей.
    pub fn session(&self, token_hash: &str, now: i64) -> Option<UserId> {
        self.sessions.get(token_hash).filter(|(_, exp)| *exp > now).map(|(id, _)| *id)
    }
    pub fn drop_session(&mut self, token_hash: &str) {
        self.sessions.remove(token_hash);
    }
    pub fn mark_verified(&mut self, user: UserId) {
        self.verified.insert(user);
    }
    pub fn is_verified(&self, user: UserId) -> bool {
        self.verified.contains(&user)
    }
    pub fn add_passkey(&mut self, user: UserId, pk: webauthn_rs::prelude::Passkey) {
        self.passkeys.entry(user).or_default().push(pk);
    }
    pub fn passkeys_of(&self, user: UserId) -> Vec<webauthn_rs::prelude::Passkey> {
        self.passkeys.get(&user).cloned().unwrap_or_default()
    }
    pub fn set_passkeys(&mut self, user: UserId, keys: Vec<webauthn_rs::prelude::Passkey>) {
        self.passkeys.insert(user, keys);
    }

    /// Идемпотентно: известный токен возвращает свой id. Флаг — создан ли новый.
    ///
    /// Нужно для демо-сида: он выполняется на каждом старте, и безусловный `create`
    /// после восстановления из БД выдавал токену НОВЫЙ id — счёт со сделками
    /// оставался в базе, но становился недоступен (поймано на рестарте).
    pub fn get_or_create(&mut self, token: String) -> (UserId, bool) {
        match self.by_token.get(&token) {
            Some(id) => (*id, false),
            None => (self.create(token), true),
        }
    }
}

// ============================ Аутентификация (ADR-018) =====================

pub(crate) const SESSION_COOKIE: &str = "session";
pub(crate) const SESSION_TTL_SECS: i64 = 30 * 24 * 3600; // 30 дней

pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Deserialize)]
pub(crate) struct CredentialsReq {
    email: String,
    password: String,
}

#[derive(Serialize)]
pub(crate) struct MeResp {
    pub(crate) user_id: u64,
    pub(crate) email: String,
    /// Подтверждена ли почта кодом. Пока нет — кабинет ограничивает операции.
    pub(crate) verified: bool,
}

/// Достать значение cookie из заголовка.
pub(crate) fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(axum::http::header::COOKIE)?.to_str().ok()?.split(';').find_map(|p| {
        let (k, v) = p.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

/// Заголовок с сессионной cookie. `HttpOnly` — чтобы её не достал JavaScript при XSS;
/// `Secure` включается переменной COOKIE_SECURE, когда сервис работает за HTTPS.
pub(crate) fn session_cookie(token: &str, max_age: i64) -> String {
    let secure = if std::env::var("COOKIE_SECURE").is_ok() { "; Secure" } else { "" };
    // Домен нужен, чтобы сессия действовала на всех поддоменах продукта
    // (accounts/trade/my), а не только там, где выполнен вход (ADR-019).
    let domain = match std::env::var("COOKIE_DOMAIN") {
        Ok(d) if !d.is_empty() => format!("; Domain={d}"),
        _ => String::new(),
    };
    format!("{SESSION_COOKIE}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}{secure}{domain}")
}

/// Скользящее окно попыток. `true` — лимит исчерпан.
pub(crate) async fn too_many(st: &AppState, key: &str, limit: usize, window: i64) -> bool {
    let now = now_unix();
    let mut map = st.attempts.lock().await;
    let hits = map.entry(key.to_string()).or_default();
    hits.retain(|t| now - *t < window);
    if hits.len() >= limit {
        return true;
    }
    hits.push(now);
    false
}

/// Адрес клиента для ограничителя. За прокси берём X-Forwarded-For.
pub(crate) fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .unwrap_or("unknown")
        .trim()
        .to_string()
}

/// Открыть сессию: положить в память, записать в хранилище, вернуть заголовок cookie.
pub(crate) async fn open_session(st: &AppState, user: UserId) -> Result<String, ApiErr> {
    let (token, token_hash) = auth::new_session_token();
    let now = now_unix();
    let expires = now + SESSION_TTL_SECS;
    st.users.lock().await.put_session(token_hash.clone(), user, expires);
    if let Err(e) = st.store.save_session(&token_hash, user, now, expires).await {
        eprintln!("[auth] сессия не сохранена: {e}");
    }
    Ok(session_cookie(&token, SESSION_TTL_SECS))
}

#[derive(Deserialize)]
pub(crate) struct EmailReq {
    email: String,
}

#[derive(Serialize)]
pub(crate) struct EmailCheckResp {
    /// Зарегистрирована ли почта: сайт по этому ответу решает, спрашивать пароль
    /// (вход) или заводить учётку (регистрация).
    registered: bool,
    /// Есть ли у пользователя привязанный Passkey — тогда предлагаем вход по ключу.
    has_passkey: bool,
}

/// Шаг 1 пошагового входа: существует ли учётка с такой почтой.
///
/// Осознанный компромисс: ответ раскрывает, зарегистрирован ли адрес. Так устроен
/// вход у большинства крупных сервисов — иначе пошаговую форму не сделать. Риск
/// (перебор адресов) гасится ограничителем: 20 проверок за 5 минут с одного IP.
pub(crate) async fn check_email(State(st): State<AppState>, headers: HeaderMap, Json(req): Json<EmailReq>) -> Result<Json<EmailCheckResp>, ApiErr> {
    if too_many(&st, &format!("check:{}", client_ip(&headers)), 20, 300).await {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too many attempts, try later"));
    }
    let email = auth::normalize_email(&req.email);
    if !auth::is_email_like(&email) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid email"));
    }
    let (registered, has_passkey) = {
        let reg = st.users.lock().await;
        match reg.find_by_email(&email) {
            Some((id, _)) => (true, !reg.passkeys_of(id).is_empty()),
            None => (false, false),
        }
    };
    Ok(Json(EmailCheckResp { registered, has_passkey }))
}

#[derive(Deserialize)]
pub(crate) struct VerifyReq {
    code: String,
}

/// Выслать код подтверждения на почту текущего пользователя.
pub(crate) async fn send_code(State(st): State<AppState>, headers: HeaderMap) -> Result<Json<serde_json::Value>, ApiErr> {
    let user = authed(&st, &headers).await?;
    if too_many(&st, &format!("code:{}", user.0), 5, 600).await {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too many attempts, try later"));
    }
    let email = st.users.lock().await.email_of(user).unwrap_or_default();
    let code = auth::new_verification_code();
    st.codes.lock().await.insert(user, (code.clone(), now_unix() + 900)); // 15 минут
    mailer::send_code(&email, &code).await;
    // Признак `delivered` показывает интерфейсу, ушло ли письмо на самом деле.
    // Без настроенного SMTP код уходит в лог сервера, и врать об этом нельзя.
    Ok(Json(serde_json::json!({ "sent": true, "delivered": mailer::is_configured() })))
}

/// Проверить код подтверждения.
pub(crate) async fn verify_code(State(st): State<AppState>, headers: HeaderMap, Json(req): Json<VerifyReq>) -> Result<Json<serde_json::Value>, ApiErr> {
    let user = authed(&st, &headers).await?;
    if too_many(&st, &format!("verify:{}", user.0), 10, 600).await {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too many attempts, try later"));
    }
    let ok = {
        let codes = st.codes.lock().await;
        codes.get(&user).is_some_and(|(c, exp)| *c == req.code.trim() && *exp > now_unix())
    };
    if !ok {
        return Err(err(StatusCode::BAD_REQUEST, "invalid or expired code"));
    }
    st.codes.lock().await.remove(&user);
    st.users.lock().await.mark_verified(user);
    Ok(Json(serde_json::json!({ "verified": true })))
}

pub(crate) async fn register(State(st): State<AppState>, headers: HeaderMap, Json(req): Json<CredentialsReq>) -> Result<impl IntoResponse, ApiErr> {
    if too_many(&st, &format!("reg:{}", client_ip(&headers)), 5, 3600).await {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too many attempts, try later"));
    }
    let email = auth::normalize_email(&req.email);
    if !auth::is_email_like(&email) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid email"));
    }
    // Требования к паролю проверяются на сервере: проверку в браузере легко обойти.
    auth::check_password_policy(&req.password).map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
    if st.users.lock().await.find_by_email(&email).is_some() {
        return Err(err(StatusCode::CONFLICT, "email already registered"));
    }
    let hash = auth::hash_password(&req.password).map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "hash failed"))?;
    let user = { st.users.lock().await.create_with_password(email.clone(), hash.clone()) };
    if let Err(e) = st.store.save_credentials(user, &email, &hash).await {
        eprintln!("[auth] учётные данные не сохранены: {e}");
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "storage unavailable"));
    }
    let ck = open_session(&st, user).await?;
    Ok(([(axum::http::header::SET_COOKIE, ck)], Json(MeResp { user_id: user.0, email, verified: false })))
}

pub(crate) async fn login(State(st): State<AppState>, headers: HeaderMap, Json(req): Json<CredentialsReq>) -> Result<impl IntoResponse, ApiErr> {
    let email = auth::normalize_email(&req.email);
    if too_many(&st, &format!("login:{}:{}", client_ip(&headers), email), 10, 300).await {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too many attempts, try later"));
    }
    let found = st.users.lock().await.find_by_email(&email);
    // Хэш проверяется всегда, даже если почты нет: иначе по времени ответа видно,
    // какие адреса зарегистрированы. Сообщение об ошибке тоже одно на оба случая.
    let (user, hash) = match found {
        Some(v) => v,
        None => (UserId(0), auth::dummy_hash().to_string()),
    };
    if !auth::verify_password(&req.password, &hash) || user.0 == 0 {
        return Err(err(StatusCode::UNAUTHORIZED, "invalid email or password"));
    }
    let ck = open_session(&st, user).await?;
    let verified = st.users.lock().await.is_verified(user);
    Ok(([(axum::http::header::SET_COOKIE, ck)], Json(MeResp { user_id: user.0, email, verified })))
}

pub(crate) async fn logout(State(st): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if let Some(token) = cookie(&headers, SESSION_COOKIE) {
        let h = auth::hash_session_token(&token);
        st.users.lock().await.drop_session(&h);
        let _ = st.store.delete_session(&h).await;
    }
    ([(axum::http::header::SET_COOKIE, session_cookie("", 0))], Json(serde_json::json!({ "ok": true })))
}

pub(crate) async fn me(State(st): State<AppState>, headers: HeaderMap) -> Result<Json<MeResp>, ApiErr> {
    let user = authed(&st, &headers).await?;
    let reg = st.users.lock().await;
    let email = reg.email_of(user).unwrap_or_default();
    Ok(Json(MeResp { user_id: user.0, email, verified: reg.is_verified(user) }))
}

/// Кто выполняет запрос: сначала сессия из cookie (ADR-018), затем — прежний
/// dev-токен `Bearer`. Легаси нужен, пока терминал переключает демо-пользователей
/// селектором; уйдёт вместе с его переездом на сессии.
pub(crate) async fn authed(st: &AppState, headers: &HeaderMap) -> Result<UserId, ApiErr> {
    if let Some(token) = cookie(headers, SESSION_COOKIE) {
        let users = st.users.lock().await;
        if let Some(user) = users.session(&auth::hash_session_token(&token), now_unix()) {
            return Ok(user);
        }
    }
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "not authenticated"))?;
    let users = st.users.lock().await;
    users.resolve(token).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "not authenticated"))
}
