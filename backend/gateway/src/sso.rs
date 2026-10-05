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
    /// Сайт, к которому привязана сессия (`tenant` из провалидированного токена).
    /// Нужен на торговых ручках: allow-list инструментов задаётся per-site, и
    /// определять сайт по чему-либо, кроме токена, нельзя — `?site=` из браузера
    /// подставит кто угодно.
    tenant: String,
    expires: Instant,
}

/// Сервис SSO: кэш JWKS, защита от повтора `jti`, стор сессий терминала.
pub struct Sso {
    /// URL JWKS платформы (`{CABINET_URL}/api/sso/jwks`). Пусто → SSO выключен.
    jwks_url: String,
    jwks: Mutex<Option<JwksCache>>,
    seen_jti: Mutex<HashMap<String, Instant>>,
    sessions: Mutex<HashMap<String, Session>>,
    /// Проверять ли `exp`. **Существует только в тестовой сборке** и только для того,
    /// чтобы остальные пункты чеклиста (tenant, one-shot jti) проверялись на настоящем
    /// токене платформы: живой токен из `sso-pack` истёк, а переподписать его нечем —
    /// приватного ключа платформы у нас нет и быть не должно. В рабочей сборке этого
    /// поля нет вовсе, поэтому `exp` проверяется всегда.
    #[cfg(test)]
    check_exp: bool,
}

impl Sso {
    pub fn new(jwks_url: String) -> Self {
        Self {
            jwks_url,
            jwks: Mutex::new(None),
            seen_jti: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            #[cfg(test)]
            check_exp: true,
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
        #[cfg(test)]
        {
            v.validate_exp = self.check_exp;
        }
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
    pub async fn mint(&self, session_token: String, user: UserId, tenant: String) -> u64 {
        let mut s = self.sessions.lock().await;
        let now = Instant::now();
        s.retain(|_, sess| sess.expires > now);
        s.insert(session_token, Session { user, tenant, expires: now + SESSION_TTL });
        SESSION_TTL.as_secs()
    }

    /// Разрешить сессию из bearer-токена → `(пользователь, сайт)`, если жива.
    pub async fn resolve(&self, token: &str) -> Option<(UserId, String)> {
        let s = self.sessions.lock().await;
        s.get(token)
            .filter(|sess| sess.expires > Instant::now())
            .map(|sess| (sess.user, sess.tenant.clone()))
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
    /// Тот же сервис, но без проверки `exp` — чтобы дойти до проверок tenant и jti
    /// на настоящем (истёкшем) токене платформы. Подпись при этом проверяется
    /// по-настоящему, реальным ключом: ослаблен ровно один пункт чеклиста, и он
    /// закрыт отдельным тестом `validate_rejects_expired`.
    fn new_ignoring_exp(jwks_url: String) -> Self {
        Self { check_exp: false, ..Self::new(jwks_url) }
    }

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

    // ==================== Чеклист sso-pack: сценарии ОТКАЗА ====================
    //
    // Пункты чеклиста (`sso-pack/README.md`): 1 — только RS256, 2 — iss/aud,
    // 3 — exp, 4 — one-shot jti, 5 — tenant == ?site=.
    //
    // Все тесты ниже идут против **настоящего ключа платформы** и настоящего токена.
    // Подделать токен нечем: приватного ключа платформы у нас нет, и это правильно.
    // Поэтому там, где нужен токен, доживший до проверки claims, отключается ровно
    // одна проверка — `exp` (`new_ignoring_exp`, существует только в тестовой сборке).
    //
    // Пункт 2 (iss/aud) юнит-тестом на отказ **не покрывается**: чтобы получить токен
    // с чужим `iss`, его надо подписать ключом платформы. Подпись проверяется до
    // claims, поэтому подмена payload'а падает раньше — на подписи, и тест доказывал
    // бы не то. Этот пункт закрывается живым прогоном TRM-04.

    /// Пункт 1: заголовок с `alg: HS256` — классическая подмена алгоритма, при которой
    /// публичный ключ пытаются использовать как HMAC-секрет. Отказ обязан наступить
    /// до любой работы с ключом.
    #[tokio::test]
    async fn rejects_alg_substitution_hs256() {
        let sso = Sso::new_ignoring_exp("http://x/jwks".into());
        sso.seed_key(KID, N, E).await;
        let parts: Vec<&str> = TOKEN.split('.').collect();
        let forged = format!("eyJhbGciOiJIUzI1NiIsImtpZCI6InBsYXRmb3JtLXNzby0xIiwidHlwIjoiSldUIn0.{}.{}", parts[1], parts[2]);
        assert!(matches!(sso.validate(&forged, "apex-ru").await, Err(SsoError::BadToken)));
    }

    /// Пункт 1: `alg: none` — токен «без подписи».
    #[tokio::test]
    async fn rejects_alg_none() {
        let sso = Sso::new_ignoring_exp("http://x/jwks".into());
        sso.seed_key(KID, N, E).await;
        let parts: Vec<&str> = TOKEN.split('.').collect();
        let forged = format!("eyJhbGciOiJub25lIiwia2lkIjoicGxhdGZvcm0tc3NvLTEiLCJ0eXAiOiJKV1QifQ.{}.", parts[1]);
        assert!(matches!(sso.validate(&forged, "apex-ru").await, Err(SsoError::BadToken)));
    }

    /// Пункт 1: другой RSA-алгоритм (RS512) тоже не принимаем — принимаем ТОЛЬКО RS256.
    #[tokio::test]
    async fn rejects_other_rsa_alg() {
        let sso = Sso::new_ignoring_exp("http://x/jwks".into());
        sso.seed_key(KID, N, E).await;
        let parts: Vec<&str> = TOKEN.split('.').collect();
        let forged = format!("eyJhbGciOiJSUzUxMiIsImtpZCI6InBsYXRmb3JtLXNzby0xIiwidHlwIjoiSldUIn0.{}.{}", parts[1], parts[2]);
        assert!(matches!(sso.validate(&forged, "apex-ru").await, Err(SsoError::BadToken)));
    }

    /// Пункт 1, отдельно: явная проверка `alg` обязана срабатывать **до** обращения к
    /// ключам. Мутационный прогон показал, что три теста выше остаются зелёными и без
    /// неё — `Validation::new(RS256)` библиотеки отвергает чужой алгоритм сама. То есть
    /// они проверяют свойство (отказ), а не наш барьер. Этот проверяет барьер: JWKS не
    /// посажен и недостижим, поэтому «дошли до ключей» видно по коду ошибки — `NoKeys`
    /// вместо `BadToken`.
    #[tokio::test]
    async fn rejects_alg_before_touching_keys() {
        let sso = Sso::new_ignoring_exp("http://127.0.0.1:9/jwks".into()); // порт-заглушка
        let parts: Vec<&str> = TOKEN.split('.').collect();
        let forged = format!("eyJhbGciOiJIUzI1NiIsImtpZCI6InBsYXRmb3JtLXNzby0xIiwidHlwIjoiSldUIn0.{}.{}", parts[1], parts[2]);
        // Разбираем вручную, а не через expect_err: тот требует Debug на Claims, а
        // печатать содержимое токена (sub/tenant/jti) в панике мы не хотим.
        match sso.validate(&forged, "apex-ru").await {
            Err(SsoError::BadToken) => {}
            Err(other) => panic!("ожидали BadToken до похода за ключами, получили {other:?}"),
            Ok(_) => panic!("подмена alg обязана быть отвергнута"),
        }
    }

    /// Пункт 5: `tenant` токена не совпадает с запрошенным сайтом → 403.
    /// Это защита от подмены сайта: токен apex-ru не должен открывать счёт на другом сайте.
    #[tokio::test]
    async fn rejects_tenant_mismatch() {
        let sso = Sso::new_ignoring_exp("http://x/jwks".into());
        sso.seed_key(KID, N, E).await;
        assert!(matches!(sso.validate(TOKEN, "other-site").await, Err(SsoError::TenantMismatch)));
    }

    /// Пункт 4: повторно предъявленный `jti` — one-shot. Первый раз проходит, второй нет.
    #[tokio::test]
    async fn rejects_replayed_jti() {
        let sso = Sso::new_ignoring_exp("http://x/jwks".into());
        sso.seed_key(KID, N, E).await;
        assert!(sso.validate(TOKEN, "apex-ru").await.is_ok(), "первое предъявление должно проходить");
        assert!(matches!(sso.validate(TOKEN, "apex-ru").await, Err(SsoError::Replay)));
    }

    /// Пункт 4, тонкое место: `jti` гасится ТОЛЬКО при полном успехе. Иначе чужой
    /// сайт, предъявив токен не туда, сжигал бы его — и настоящий вход ломался бы
    /// отказом «уже использован». Проверяем, что отказ по tenant не съедает jti.
    #[tokio::test]
    async fn failed_validation_does_not_consume_jti() {
        let sso = Sso::new_ignoring_exp("http://x/jwks".into());
        sso.seed_key(KID, N, E).await;
        assert!(matches!(sso.validate(TOKEN, "other-site").await, Err(SsoError::TenantMismatch)));
        assert!(sso.validate(TOKEN, "apex-ru").await.is_ok(), "неудачная попытка не должна гасить jti");
    }

    /// Сессия: выпуск → разрешение → логаут.
    #[tokio::test]
    async fn session_lifecycle() {
        let sso = Sso::new("http://x/jwks".into());
        sso.mint("tok-123".into(), UserId(42), "apex-ru".into()).await;
        assert_eq!(sso.resolve("tok-123").await, Some((UserId(42), "apex-ru".to_string())));
        sso.logout("tok-123").await;
        assert_eq!(sso.resolve("tok-123").await, None);
    }

    /// Сайт берётся из сессии, а значит из провалидированного токена. Проверяем, что
    /// он действительно доезжает до потребителя: на нём стоит allow-list инструментов.
    #[tokio::test]
    async fn session_carries_tenant() {
        let sso = Sso::new("http://x/jwks".into());
        sso.mint("a".into(), UserId(1), "apex-ru".into()).await;
        sso.mint("b".into(), UserId(2), "other-site".into()).await;
        assert_eq!(sso.resolve("a").await.map(|(_, t)| t), Some("apex-ru".to_string()));
        assert_eq!(sso.resolve("b").await.map(|(_, t)| t), Some("other-site".to_string()));
    }
}
