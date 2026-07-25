//! # sso — вход по handoff-токену платформы (ADR-023, фаза Т2)
//!
//! Терминал встраивается в кабинет сайта (iframe). Модель (sso-pack v1):
//! платформа выдаёт короткоживущий **one-shot JWT** (RS256), кабинет передаёт его в
//! iframe через `postMessage`; терминал валидирует токен по **JWKS** платформы и чеканит
//! **свою** сессию-bearer (TTL ≤ 15 мин), которую фронт держит в памяти и продлевает
//! ре-handoff'ом. Куки не используем: в iframe это third-party cookie — их режут браузеры.
//!
//! Валидация (всё обязательно): подпись по JWKS, только `alg=RS256`, `iss=platform-auth`,
//! `aud=terminal`, `exp` (допуск ±30 с), one-shot `jti`, `tenant == ?site=`.
//! Счёт — по ключу `(tenant, sub)` из провалидированного токена.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::Mutex;

use domain::account::UserId;

const SESSION_TTL: Duration = Duration::from_secs(15 * 60);
const JWKS_TTL: Duration = Duration::from_secs(60 * 60);
const LEEWAY_SECS: u64 = 30;
/// Сколько помним использованные `jti` — с запасом больше окна `exp`+допуск.
const JTI_TTL: Duration = Duration::from_secs(120);

/// Полезная нагрузка handoff-токена. `iss`/`aud`/`exp` проверяет `Validation` из сырого
/// токена (в структуре их держать не нужно), нам нужны `sub`/`tenant`/`jti`.
#[derive(Deserialize)]
pub struct Claims {
    pub sub: String,
    pub tenant: String,
    pub jti: String,
}

/// Ошибка входа. Наверх маппится в HTTP-код (см. `code()`).
#[derive(Debug)]
pub enum SsoError {
    BadToken,     // 401: подпись/срок/claims/формат
    TenantMismatch, // 403: tenant токена != ?site=
    Replay,       // 401: jti уже использован
    NoKeys,       // 503: JWKS недоступен
    Disabled,     // 501: SSO выключен, а сессия запрошена
}
impl SsoError {
    pub fn as_str(&self) -> &'static str {
        match self {
            SsoError::BadToken => "invalid token",
            SsoError::TenantMismatch => "tenant does not match site",
            SsoError::Replay => "token already used",
            SsoError::NoKeys => "auth keys unavailable",
            SsoError::Disabled => "sso disabled",
        }
    }
}

struct JwksCache {
    keys: HashMap<String, DecodingKey>,
    fetched: Instant,
}

struct Session {
    user: UserId,
    expires: Instant,
}

/// Сервис SSO: кэш JWKS, защита от повтора `jti`, стор сессий терминала.
pub struct Sso {
    /// URL JWKS платформы (`{CABINET_URL}/api/sso/jwks`). Пусто → SSO выключен.
    jwks_url: String,
    jwks: Mutex<Option<JwksCache>>,
    seen_jti: Mutex<HashMap<String, Instant>>,
    sessions: Mutex<HashMap<String, Session>>,
}

impl Sso {
    pub fn new(jwks_url: String) -> Self {
        Self {
            jwks_url,
            jwks: Mutex::new(None),
            seen_jti: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn enabled(&self) -> bool {
        !self.jwks_url.is_empty()
    }

    /// Достать ключ по `kid`: из кэша, иначе (или если кэш протух) — обновить JWKS.
    async fn key_for(&self, kid: &str) -> Result<DecodingKey, SsoError> {
        {
            let cache = self.jwks.lock().await;
            if let Some(c) = cache.as_ref() {
                if c.fetched.elapsed() < JWKS_TTL {
                    if let Some(k) = c.keys.get(kid) {
                        return Ok(k.clone());
                    }
                }
            }
        }
        // Кэша нет / протух / неизвестный kid (возможна ротация) — тянем заново.
        let fresh = self.fetch_jwks().await?;
        let key = fresh.get(kid).cloned();
        *self.jwks.lock().await = Some(JwksCache { keys: fresh, fetched: Instant::now() });
        key.ok_or(SsoError::BadToken)
    }

    async fn fetch_jwks(&self) -> Result<HashMap<String, DecodingKey>, SsoError> {
        let resp = reqwest::get(&self.jwks_url).await.map_err(|_| SsoError::NoKeys)?;
        if !resp.status().is_success() {
            return Err(SsoError::NoKeys);
        }
        let doc: JwksDoc = resp.json().await.map_err(|_| SsoError::NoKeys)?;
        let mut out = HashMap::new();
        for k in doc.keys {
            // Берём только подписные RSA-ключи; DecodingKey строится из компонент n,e.
            if k.kty == "RSA" && k.alg.as_deref().unwrap_or("RS256") == "RS256" {
                if let Ok(key) = DecodingKey::from_rsa_components(&k.n, &k.e) {
                    out.insert(k.kid, key);
                }
            }
        }
        if out.is_empty() {
            return Err(SsoError::NoKeys);
        }
        Ok(out)
    }

    /// Провалидировать handoff-токен под ожидаемый сайт. Возвращает claims.
    /// Помечает `jti` использованным (one-shot) только при полном успехе.
    pub async fn validate(&self, token: &str, expected_site: &str) -> Result<Claims, SsoError> {
        if !self.enabled() {
            return Err(SsoError::Disabled);
        }
        let header = decode_header(token).map_err(|_| SsoError::BadToken)?;
        if header.alg != Algorithm::RS256 {
            return Err(SsoError::BadToken); // защита от alg-подмены (none/HS*)
        }
        let kid = header.kid.ok_or(SsoError::BadToken)?;
        let key = self.key_for(&kid).await?;

        let mut v = Validation::new(Algorithm::RS256);
        v.leeway = LEEWAY_SECS;
        v.set_issuer(&["platform-auth"]);
        v.set_audience(&["terminal"]);
        v.set_required_spec_claims(&["exp", "aud", "iss"]);
        let data = decode::<Claims>(token, &key, &v).map_err(|_| SsoError::BadToken)?;
        let claims = data.claims;

        if claims.tenant != expected_site {
            return Err(SsoError::TenantMismatch);
        }
        // One-shot: атомарно проверить и пометить jti (под одним локом).
        {
            let mut seen = self.seen_jti.lock().await;
            let now = Instant::now();
            seen.retain(|_, t| now.duration_since(*t) < JTI_TTL);
            if seen.contains_key(&claims.jti) {
                return Err(SsoError::Replay);
            }
            seen.insert(claims.jti.clone(), now);
        }
        Ok(claims)
    }

    /// Выпустить сессию терминала для пользователя. Токен сессии — сам `jti`
    /// (UUIDv4, 122 бит энтропии, одноразовый и уникальный) — отдельный RNG не нужен.
    pub async fn mint(&self, session_token: String, user: UserId) -> u64 {
        let mut s = self.sessions.lock().await;
        let now = Instant::now();
        s.retain(|_, sess| sess.expires > now);
        s.insert(session_token, Session { user, expires: now + SESSION_TTL });
        SESSION_TTL.as_secs()
    }

    /// Разрешить сессию из bearer-токена → UserId (если жива).
    pub async fn resolve(&self, token: &str) -> Option<UserId> {
        let s = self.sessions.lock().await;
        s.get(token).filter(|sess| sess.expires > Instant::now()).map(|sess| sess.user)
    }

    /// Погасить сессию (логаут).
    pub async fn logout(&self, token: &str) {
        self.sessions.lock().await.remove(token);
    }
}

#[derive(Deserialize)]
struct JwksDoc {
    keys: Vec<Jwk>,
}
#[derive(Deserialize)]
struct Jwk {
    kty: String,
    #[serde(default)]
    alg: Option<String>,
    kid: String,
    n: String,
    e: String,
}

#[cfg(test)]
impl Sso {
    /// Тестовый шов: посадить ключ в кэш JWKS, чтобы `validate` не ходил в сеть.
    async fn seed_key(&self, kid: &str, n: &str, e: &str) {
        let key = DecodingKey::from_rsa_components(n, e).unwrap();
        let mut keys = HashMap::new();
        keys.insert(kid.to_string(), key);
        *self.jwks.lock().await = Some(JwksCache { keys, fetched: Instant::now() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Живой публичный ключ и токен из sso-pack (токен истёк — для проверки парсера/подписи).
    const N: &str = "sGSxmuUt9pN-Zq_90le5ilc7YC_fhZtOE7S9fSp5X69xZaQfx3LTpw4E6re-J2cFQxXRNX-cEYJX4Slbr6fxW02bouMTiuBfT0GV5-oZhN8T4Yxgo0yNjeKCtk2nII2ECyobQfSv2yINpQ_1U-NPrUiwY3ZkYI6_g-xpgHhoKbLMWcf_muE37St5hqX_mrAHj3983jlAdq2BnNhSrvUg9KM3SAXkQg6cTiXYZfAYyDaEiP02DIJTgfpB84Bd1lvI05jQ8ltzKKkU_uowiNhW_qy5K9xp_oIFGs0gpRi6dNxMIJvwteBbtbBS0LpJY4jqeU5mv63ZtFVpOXhFS5XfXQ";
    const E: &str = "AQAB";
    const KID: &str = "platform-sso-1";
    const TOKEN: &str = "eyJhbGciOiJSUzI1NiIsImtpZCI6InBsYXRmb3JtLXNzby0xIiwidHlwIjoiSldUIn0.eyJ0ZW5hbnQiOiJhcGV4LXJ1IiwiaXNzIjoicGxhdGZvcm0tYXV0aCIsImF1ZCI6InRlcm1pbmFsIiwic3ViIjoiMGNkMWIzODMtYzliZC00MDY2LTk5MDItMmQ0NTRhZjcxMWVlIiwianRpIjoiZDk5ZTcyN2QtMmQzYi00ZWZhLWI3Y2ItMjgyMDBmYjYxN2ViIiwiaWF0IjoxNzg0OTc5NDk3LCJleHAiOjE3ODQ5Nzk1NTd9.GYDv6XuLJi-pF6trCBw4a8N0j-NybykekxoK4WmsjktNg5kF2iouG7TBmwqWorAmYD5lWkWDfSUubtoTdxWFhbOJt0GipfoE3Z8MyPFQAwgwSlIUJeK5LzVC7Jduf4t7Oni-YhFIoQTkzMJwBf2KoWQpoHqE4UThNFVPQBdrVLaLo7kWFhSqU5guNcL8ZtMuQ1MCd_2EUdN2tw8kZu2mo0lo5XdiDs1e_kMu8yasLTfGerY6Q8I2Mx5FAbruKzB_UPwY4A67IhR-2hFsxq1Dhjt2rHIVtl562SDDpUfapaY6ft16gt_Ag4eHI03keBbmxOfaJPq8gN3nrmjj2oviJA";

    /// Реальный публичный ключ платформы верифицирует реальный токен, и мы читаем claims
    /// (exp не проверяем — токен из пакета истёк; проверяем именно подпись+парсинг).
    #[test]
    fn real_key_verifies_real_token() {
        let key = DecodingKey::from_rsa_components(N, E).unwrap();
        let mut v = Validation::new(Algorithm::RS256);
        v.validate_exp = false;
        v.set_issuer(&["platform-auth"]);
        v.set_audience(&["terminal"]);
        let data = decode::<Claims>(TOKEN, &key, &v).expect("подпись должна сойтись реальным ключом");
        assert_eq!(data.claims.tenant, "apex-ru");
        assert_eq!(data.claims.sub, "0cd1b383-c9bd-4066-9902-2d454af711ee");
        assert_eq!(data.claims.jti, "d99e727d-2d3b-4efa-b7cb-28200fb617eb");
    }

    /// `validate` с реальным ключом: токен истёк → отказ (полный путь, без сети).
    #[tokio::test]
    async fn validate_rejects_expired() {
        let sso = Sso::new("http://x/jwks".into());
        sso.seed_key(KID, N, E).await;
        assert!(matches!(sso.validate(TOKEN, "apex-ru").await, Err(SsoError::BadToken)));
    }

    /// Мусор вместо JWT → отказ, не паника.
    #[tokio::test]
    async fn validate_rejects_garbage() {
        let sso = Sso::new("http://x/jwks".into());
        sso.seed_key(KID, N, E).await;
        assert!(matches!(sso.validate("not-a-jwt", "apex-ru").await, Err(SsoError::BadToken)));
    }

    /// Без URL JWKS SSO выключен — запрос сессии отклоняется как Disabled.
    #[tokio::test]
    async fn disabled_without_url() {
        let sso = Sso::new(String::new());
        assert!(!sso.enabled());
        assert!(matches!(sso.validate(TOKEN, "apex-ru").await, Err(SsoError::Disabled)));
    }

    /// Сессия: выпуск → разрешение → логаут.
    #[tokio::test]
    async fn session_lifecycle() {
        let sso = Sso::new("http://x/jwks".into());
        sso.mint("tok-123".into(), UserId(42)).await;
        assert_eq!(sso.resolve("tok-123").await, Some(UserId(42)));
        sso.logout("tok-123").await;
        assert_eq!(sso.resolve("tok-123").await, None);
    }
}
