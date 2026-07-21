//! # auth
//!
//! Пароли и токены сессий (ADR-018). Крейт ничего не знает ни про БД, ни про HTTP:
//! наружу отдаются строки, а хранит их адаптер (`persistence` + `gateway`).
//!
//! - **Пароль** хэшируется **Argon2id** (memory-hard, рекомендация OWASP). Результат —
//!   стандартная PHC-строка: соль и параметры лежат рядом с хэшем, поэтому стойкость
//!   можно поднять позже, не ломая старые записи.
//! - **Токен сессии** — 32 случайных байта из `OsRng` в base64url. В хранилище кладётся
//!   **SHA-256 от токена**: утечка базы не даёт войти. Быстрый хэш здесь уместен —
//!   токен высокоэнтропийный, перебирать его бессмысленно (в отличие от пароля).

use argon2::Argon2;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Минимальные требования к паролю. Держим здесь, чтобы сервер не полагался
/// на проверку в браузере — её можно обойти.
pub const MIN_PASSWORD_LEN: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordError {
    TooShort,
    NeedsLetterAndDigit,
}

impl std::fmt::Display for PasswordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PasswordError::TooShort => write!(f, "password must be at least {MIN_PASSWORD_LEN} characters"),
            PasswordError::NeedsLetterAndDigit => write!(f, "password must contain a letter and a digit"),
        }
    }
}

/// Проверить требования к паролю до хэширования.
pub fn check_password_policy(password: &str) -> Result<(), PasswordError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(PasswordError::TooShort);
    }
    let has_letter = password.chars().any(|c| c.is_alphabetic());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    if !has_letter || !has_digit {
        return Err(PasswordError::NeedsLetterAndDigit);
    }
    Ok(())
}

/// Захэшировать пароль. Соль генерируется на каждый вызов, поэтому одинаковые
/// пароли дают разные хэши.
pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

/// Проверить пароль против сохранённого хэша. Любая ошибка разбора хэша — это
/// «не подошёл», наружу подробности не уходят.
pub fn verify_password(password: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(parsed) => Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok(),
        Err(_) => false,
    }
}

/// Хэш-заглушка для несуществующего пользователя.
///
/// При входе с незарегистрированной почтой сервер всё равно обязан потратить время
/// на проверку: иначе по скорости ответа видно, какие адреса зарегистрированы.
pub fn dummy_hash() -> &'static str {
    // Заранее посчитанный Argon2id от случайной строки; подойти не может ни один пароль.
    "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$Yk7oPPqJ7CvJ0h5nQZK2Yy1WQmZ4vJqL8h3xNn0aB1c"
}

/// Новый токен сессии: (сам токен для клиента, его хэш для хранилища).
pub fn new_session_token() -> (String, String) {
    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    let token = URL_SAFE_NO_PAD.encode(raw);
    let hash = hash_session_token(&token);
    (token, hash)
}

/// Хэш токена сессии — то, что лежит в БД и по чему ищется сессия.
pub fn hash_session_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

/// Привести почту к каноничному виду для поиска и проверки уникальности:
/// регистр в адресах не значим, пробелы по краям — опечатка.
pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

/// Грубая проверка формата почты. Это не валидация по RFC — она невозможна без
/// отправки письма; задача только отсечь очевидный мусор.
pub fn is_email_like(email: &str) -> bool {
    let e = email.trim();
    if e.len() < 5 || e.len() > 254 || e.contains(char::is_whitespace) {
        return false;
    }
    let Some((local, domain)) = e.split_once('@') else { return false };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let phc = hash_password("Str0ngPass1").unwrap();
        assert!(verify_password("Str0ngPass1", &phc));
        assert!(!verify_password("Str0ngPass2", &phc));
    }

    #[test]
    fn same_password_gets_different_hashes() {
        // Соль на каждый вызов: одинаковые пароли не должны выглядеть одинаково в БД.
        let a = hash_password("Str0ngPass1").unwrap();
        let b = hash_password("Str0ngPass1").unwrap();
        assert_ne!(a, b);
        assert!(verify_password("Str0ngPass1", &a) && verify_password("Str0ngPass1", &b));
    }

    #[test]
    fn hash_is_argon2id_and_hides_password() {
        let phc = hash_password("Str0ngPass1").unwrap();
        assert!(phc.starts_with("$argon2id$"), "должен быть Argon2id, а не быстрый хэш");
        assert!(!phc.contains("Str0ngPass1"));
    }

    #[test]
    fn broken_hash_is_not_a_match() {
        assert!(!verify_password("whatever", "не-PHC-строка"));
        assert!(!verify_password("whatever", ""));
    }

    #[test]
    fn dummy_hash_never_matches_but_parses() {
        // Должен именно проверяться (тратить время), а не отваливаться на разборе.
        assert!(PasswordHash::new(dummy_hash()).is_ok(), "заглушка должна быть валидной PHC-строкой");
        assert!(!verify_password("Str0ngPass1", dummy_hash()));
    }

    #[test]
    fn policy_rejects_weak_passwords() {
        assert_eq!(check_password_policy("short1"), Err(PasswordError::TooShort));
        assert_eq!(check_password_policy("onlyletters"), Err(PasswordError::NeedsLetterAndDigit));
        assert_eq!(check_password_policy("12345678"), Err(PasswordError::NeedsLetterAndDigit));
        assert!(check_password_policy("Str0ngPass1").is_ok());
    }

    #[test]
    fn session_tokens_are_unique_and_stored_hashed() {
        let (t1, h1) = new_session_token();
        let (t2, h2) = new_session_token();
        assert_ne!(t1, t2, "два вызова не должны давать одинаковый токен");
        assert_ne!(t1, h1, "в БД кладётся хэш, а не сам токен");
        assert_ne!(h1, h2);
        assert_eq!(hash_session_token(&t1), h1, "хэш воспроизводим — по нему ищем сессию");
        assert!(t1.len() >= 40, "32 байта энтропии");
    }

    #[test]
    fn email_normalization_and_shape() {
        assert_eq!(normalize_email("  Trader@Example.COM "), "trader@example.com");
        assert!(is_email_like("trader@example.com"));
        assert!(!is_email_like("not-an-email"));
        assert!(!is_email_like("a@b"));
        assert!(!is_email_like("тр ейдер@example.com"));
        assert!(!is_email_like("@example.com"));
    }
}
