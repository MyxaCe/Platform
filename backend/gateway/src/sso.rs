use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::Mutex;

use domain::account::UserId;

const SESSION_TTL: Duration = Duration::from_secs(15 * 60);
const JWKS_TTL: Duration = Duration::from_secs(60 * 60);
const LEEWAY_SECS: u64 = 30;
const JTI_TTL: Duration = Duration::from_secs(120);

#[derive(Deserialize)]
pub struct Claims {
    pub sub: String,
    pub tenant: String,
    pub jti: String,
    #[serde(default)]
    pub account_kind: Option<String>,
    #[serde(default)]
    pub leverage: Option<i64>,
    #[serde(default)]
    pub leverage_base: Option<i64>,
    #[serde(default)]
    pub leverage_classes: Option<BTreeMap<String, i64>>,
    #[serde(default)]
    pub markup_bps: Option<i64>,
    #[serde(default)]
    pub commission_bps: Option<i64>,
}

#[derive(Debug)]
pub enum SsoError {
    BadToken,
    TenantMismatch,
    Replay,
    NoKeys,
    Disabled,
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

fn reject_reason(kind: &jsonwebtoken::errors::ErrorKind) -> &'static str {
    use jsonwebtoken::errors::ErrorKind as K;
    match kind {
        K::InvalidSignature => "подпись не сошлась",
        K::InvalidIssuer => "чужой iss (токен выпущен не платформой)",
        K::InvalidAudience => "чужой aud (токен выпущен не для терминала)",
        K::ExpiredSignature => "истёк exp",
        K::ImmatureSignature => "ещё не вступил в силу (nbf)",
        K::MissingRequiredClaim(_) => "нет обязательного claim",
        K::InvalidAlgorithm => "алгоритм не RS256",
        K::InvalidToken | K::Base64(_) | K::Json(_) | K::Utf8(_) => "токен не разобран",
        _ => "прочее",
    }
}

struct JwksCache {
    keys: HashMap<String, DecodingKey>,
    fetched: Instant,
}

struct Session {
    user: UserId,
    tenant: String,
    expires: Instant,
}

pub struct Sso {
    jwks_url: String,
    jwks: Mutex<Option<JwksCache>>,
    seen_jti: Mutex<HashMap<String, Instant>>,
    sessions: Mutex<HashMap<String, Session>>,
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
        let fresh = self.fetch_jwks().await?;
        let key = fresh.get(kid).cloned();
        *self.jwks.lock().await = Some(JwksCache {
            keys: fresh,
            fetched: Instant::now(),
        });
        key.ok_or(SsoError::BadToken)
    }

    async fn fetch_jwks(&self) -> Result<HashMap<String, DecodingKey>, SsoError> {
        let resp = reqwest::get(&self.jwks_url)
            .await
            .map_err(|_| SsoError::NoKeys)?;
        if !resp.status().is_success() {
            return Err(SsoError::NoKeys);
        }
        let doc: JwksDoc = resp.json().await.map_err(|_| SsoError::NoKeys)?;
        let mut out = HashMap::new();
        for k in doc.keys {
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

    pub async fn validate(&self, token: &str, expected_site: &str) -> Result<Claims, SsoError> {
        if !self.enabled() {
            return Err(SsoError::Disabled);
        }
        let header = decode_header(token).map_err(|_| SsoError::BadToken)?;
        if header.alg != Algorithm::RS256 {
            return Err(SsoError::BadToken);
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
        let data = decode::<Claims>(token, &key, &v).map_err(|e| {
            eprintln!("[sso] handoff-токен отвергнут: {}", reject_reason(e.kind()));
            SsoError::BadToken
        })?;
        let claims = data.claims;

        if claims.tenant != expected_site {
            return Err(SsoError::TenantMismatch);
        }
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

    pub async fn mint(&self, session_token: String, user: UserId, tenant: String) -> u64 {
        let mut s = self.sessions.lock().await;
        let now = Instant::now();
        s.retain(|_, sess| sess.expires > now);
        s.insert(
            session_token,
            Session {
                user,
                tenant,
                expires: now + SESSION_TTL,
            },
        );
        SESSION_TTL.as_secs()
    }

    pub async fn resolve(&self, token: &str) -> Option<(UserId, String)> {
        let s = self.sessions.lock().await;
        s.get(token)
            .filter(|sess| sess.expires > Instant::now())
            .map(|sess| (sess.user, sess.tenant.clone()))
    }

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
    fn new_ignoring_exp(jwks_url: String) -> Self {
        Self {
            check_exp: false,
            ..Self::new(jwks_url)
        }
    }

    async fn seed_key(&self, kid: &str, n: &str, e: &str) {
        let key = DecodingKey::from_rsa_components(n, e).unwrap();
        let mut keys = HashMap::new();
        keys.insert(kid.to_string(), key);
        *self.jwks.lock().await = Some(JwksCache {
            keys,
            fetched: Instant::now(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: &str = "sGSxmuUt9pN-Zq_90le5ilc7YC_fhZtOE7S9fSp5X69xZaQfx3LTpw4E6re-J2cFQxXRNX-cEYJX4Slbr6fxW02bouMTiuBfT0GV5-oZhN8T4Yxgo0yNjeKCtk2nII2ECyobQfSv2yINpQ_1U-NPrUiwY3ZkYI6_g-xpgHhoKbLMWcf_muE37St5hqX_mrAHj3983jlAdq2BnNhSrvUg9KM3SAXkQg6cTiXYZfAYyDaEiP02DIJTgfpB84Bd1lvI05jQ8ltzKKkU_uowiNhW_qy5K9xp_oIFGs0gpRi6dNxMIJvwteBbtbBS0LpJY4jqeU5mv63ZtFVpOXhFS5XfXQ";
    const E: &str = "AQAB";
    const KID: &str = "platform-sso-1";
    const TOKEN: &str = "eyJhbGciOiJSUzI1NiIsImtpZCI6InBsYXRmb3JtLXNzby0xIiwidHlwIjoiSldUIn0.eyJ0ZW5hbnQiOiJhcGV4LXJ1IiwiaXNzIjoicGxhdGZvcm0tYXV0aCIsImF1ZCI6InRlcm1pbmFsIiwic3ViIjoiMGNkMWIzODMtYzliZC00MDY2LTk5MDItMmQ0NTRhZjcxMWVlIiwianRpIjoiZDk5ZTcyN2QtMmQzYi00ZWZhLWI3Y2ItMjgyMDBmYjYxN2ViIiwiaWF0IjoxNzg0OTc5NDk3LCJleHAiOjE3ODQ5Nzk1NTd9.GYDv6XuLJi-pF6trCBw4a8N0j-NybykekxoK4WmsjktNg5kF2iouG7TBmwqWorAmYD5lWkWDfSUubtoTdxWFhbOJt0GipfoE3Z8MyPFQAwgwSlIUJeK5LzVC7Jduf4t7Oni-YhFIoQTkzMJwBf2KoWQpoHqE4UThNFVPQBdrVLaLo7kWFhSqU5guNcL8ZtMuQ1MCd_2EUdN2tw8kZu2mo0lo5XdiDs1e_kMu8yasLTfGerY6Q8I2Mx5FAbruKzB_UPwY4A67IhR-2hFsxq1Dhjt2rHIVtl562SDDpUfapaY6ft16gt_Ag4eHI03keBbmxOfaJPq8gN3nrmjj2oviJA";

    #[test]
    fn real_key_verifies_real_token() {
        let key = DecodingKey::from_rsa_components(N, E).unwrap();
        let mut v = Validation::new(Algorithm::RS256);
        v.validate_exp = false;
        v.set_issuer(&["platform-auth"]);
        v.set_audience(&["terminal"]);
        let data =
            decode::<Claims>(TOKEN, &key, &v).expect("подпись должна сойтись реальным ключом");
        assert_eq!(data.claims.tenant, "apex-ru");
        assert_eq!(data.claims.sub, "0cd1b383-c9bd-4066-9902-2d454af711ee");
        assert_eq!(data.claims.jti, "d99e727d-2d3b-4efa-b7cb-28200fb617eb");
    }

    #[tokio::test]
    async fn validate_rejects_expired() {
        let sso = Sso::new("http://x/jwks".into());
        sso.seed_key(KID, N, E).await;
        assert!(matches!(
            sso.validate(TOKEN, "apex-ru").await,
            Err(SsoError::BadToken)
        ));
    }

    #[tokio::test]
    async fn validate_rejects_garbage() {
        let sso = Sso::new("http://x/jwks".into());
        sso.seed_key(KID, N, E).await;
        assert!(matches!(
            sso.validate("not-a-jwt", "apex-ru").await,
            Err(SsoError::BadToken)
        ));
    }

    #[tokio::test]
    async fn disabled_without_url() {
        let sso = Sso::new(String::new());
        assert!(!sso.enabled());
        assert!(matches!(
            sso.validate(TOKEN, "apex-ru").await,
            Err(SsoError::Disabled)
        ));
    }

    #[tokio::test]
    async fn rejects_alg_substitution_hs256() {
        let sso = Sso::new_ignoring_exp("http://x/jwks".into());
        sso.seed_key(KID, N, E).await;
        let parts: Vec<&str> = TOKEN.split('.').collect();
        let forged = format!(
            "eyJhbGciOiJIUzI1NiIsImtpZCI6InBsYXRmb3JtLXNzby0xIiwidHlwIjoiSldUIn0.{}.{}",
            parts[1], parts[2]
        );
        assert!(matches!(
            sso.validate(&forged, "apex-ru").await,
            Err(SsoError::BadToken)
        ));
    }

    #[tokio::test]
    async fn rejects_alg_none() {
        let sso = Sso::new_ignoring_exp("http://x/jwks".into());
        sso.seed_key(KID, N, E).await;
        let parts: Vec<&str> = TOKEN.split('.').collect();
        let forged = format!(
            "eyJhbGciOiJub25lIiwia2lkIjoicGxhdGZvcm0tc3NvLTEiLCJ0eXAiOiJKV1QifQ.{}.",
            parts[1]
        );
        assert!(matches!(
            sso.validate(&forged, "apex-ru").await,
            Err(SsoError::BadToken)
        ));
    }

    #[tokio::test]
    async fn rejects_other_rsa_alg() {
        let sso = Sso::new_ignoring_exp("http://x/jwks".into());
        sso.seed_key(KID, N, E).await;
        let parts: Vec<&str> = TOKEN.split('.').collect();
        let forged = format!(
            "eyJhbGciOiJSUzUxMiIsImtpZCI6InBsYXRmb3JtLXNzby0xIiwidHlwIjoiSldUIn0.{}.{}",
            parts[1], parts[2]
        );
        assert!(matches!(
            sso.validate(&forged, "apex-ru").await,
            Err(SsoError::BadToken)
        ));
    }

    #[tokio::test]
    async fn rejects_alg_before_touching_keys() {
        let sso = Sso::new_ignoring_exp("http://127.0.0.1:9/jwks".into());
        let parts: Vec<&str> = TOKEN.split('.').collect();
        let forged = format!(
            "eyJhbGciOiJIUzI1NiIsImtpZCI6InBsYXRmb3JtLXNzby0xIiwidHlwIjoiSldUIn0.{}.{}",
            parts[1], parts[2]
        );
        match sso.validate(&forged, "apex-ru").await {
            Err(SsoError::BadToken) => {}
            Err(other) => panic!("ожидали BadToken до похода за ключами, получили {other:?}"),
            Ok(_) => panic!("подмена alg обязана быть отвергнута"),
        }
    }

    #[tokio::test]
    async fn rejects_tenant_mismatch() {
        let sso = Sso::new_ignoring_exp("http://x/jwks".into());
        sso.seed_key(KID, N, E).await;
        assert!(matches!(
            sso.validate(TOKEN, "other-site").await,
            Err(SsoError::TenantMismatch)
        ));
    }

    #[tokio::test]
    async fn rejects_replayed_jti() {
        let sso = Sso::new_ignoring_exp("http://x/jwks".into());
        sso.seed_key(KID, N, E).await;
        assert!(
            sso.validate(TOKEN, "apex-ru").await.is_ok(),
            "первое предъявление должно проходить"
        );
        assert!(matches!(
            sso.validate(TOKEN, "apex-ru").await,
            Err(SsoError::Replay)
        ));
    }

    #[tokio::test]
    async fn failed_validation_does_not_consume_jti() {
        let sso = Sso::new_ignoring_exp("http://x/jwks".into());
        sso.seed_key(KID, N, E).await;
        assert!(matches!(
            sso.validate(TOKEN, "other-site").await,
            Err(SsoError::TenantMismatch)
        ));
        assert!(
            sso.validate(TOKEN, "apex-ru").await.is_ok(),
            "неудачная попытка не должна гасить jti"
        );
    }

    #[tokio::test]
    async fn session_lifecycle() {
        let sso = Sso::new("http://x/jwks".into());
        sso.mint("tok-123".into(), UserId(42), "apex-ru".into())
            .await;
        assert_eq!(
            sso.resolve("tok-123").await,
            Some((UserId(42), "apex-ru".to_string()))
        );
        sso.logout("tok-123").await;
        assert_eq!(sso.resolve("tok-123").await, None);
    }

    #[tokio::test]
    async fn session_carries_tenant() {
        let sso = Sso::new("http://x/jwks".into());
        sso.mint("a".into(), UserId(1), "apex-ru".into()).await;
        sso.mint("b".into(), UserId(2), "other-site".into()).await;
        assert_eq!(
            sso.resolve("a").await.map(|(_, t)| t),
            Some("apex-ru".to_string())
        );
        assert_eq!(
            sso.resolve("b").await.map(|(_, t)| t),
            Some("other-site".to_string())
        );
    }
}
