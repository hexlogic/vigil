//! Session tokens and the cookie that carries them.
//!
//! The browser holds a random token; the database holds only its hash, so a copy of the
//! database cannot be used to sign in.

use axum::http::header::COOKIE;
use axum::http::{HeaderMap, HeaderValue};

use crate::config::WebConfig;

/// Name of the session cookie.
pub(crate) const COOKIE_NAME: &str = "vigil_session";

/// How long a session lasts after it was last used, in milliseconds: 30 days.
pub(crate) const LIFETIME_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// How long a session may go unrecorded as used, in milliseconds. Recording each request
/// would write to the database on every click.
pub(crate) const TOUCH_AFTER_MS: i64 = 60 * 60 * 1000;

/// Characters in a token: 32 random bytes in hex.
const TOKEN_LEN: usize = 64;

/// A new random token.
pub(crate) fn new_token() -> Result<String, getrandom::Error> {
    let mut bytes = [0u8; TOKEN_LEN / 2];
    getrandom::fill(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// What the database knows a token by.
pub(crate) fn hash_token(token: &str) -> [u8; 32] {
    *blake3::hash(token.as_bytes()).as_bytes()
}

/// The session token the request carries, if it looks like one.
pub(crate) fn token_of(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .filter_map(|cookie| cookie.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE_NAME)
        .map(|(_, value)| value)
        .filter(|value| value.len() == TOKEN_LEN && value.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The `Set-Cookie` value that starts a session.
pub(crate) fn set_cookie(web: &WebConfig, token: &str) -> Option<HeaderValue> {
    cookie(web, token, LIFETIME_MS / 1000)
}

/// The `Set-Cookie` value that removes the session cookie.
pub(crate) fn clear_cookie(web: &WebConfig) -> Option<HeaderValue> {
    cookie(web, "", 0)
}

/// The `Set-Cookie` value that remembers the language a person picked, for a year.
pub(crate) fn language_cookie(web: &WebConfig, code: &str) -> Option<HeaderValue> {
    let path = web.base_path();
    let path = if path.is_empty() { "/" } else { &path };
    let secure = if web.secure_cookies() { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{}={code}; Path={path}; Max-Age=31536000; SameSite=Lax{secure}",
        crate::i18n::LANG_COOKIE
    ))
    .ok()
}

fn cookie(web: &WebConfig, value: &str, max_age: i64) -> Option<HeaderValue> {
    let path = web.base_path();
    let path = if path.is_empty() { "/" } else { &path };
    let secure = if web.secure_cookies() { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}={value}; Path={path}; Max-Age={max_age}; HttpOnly; SameSite=Lax{secure}"
    ))
    .ok()
}

#[cfg(test)]
mod tests {
    use crate::config::Auth;

    use super::*;

    #[test]
    fn tokens_are_long_random_and_distinct() {
        let (a, b) = (new_token().expect("random"), new_token().expect("random"));
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert_ne!(hash_token(&a), hash_token(&b));
        assert_eq!(hash_token(&a), hash_token(&a));
    }

    #[test]
    fn only_a_well_formed_token_is_read_from_the_cookie() {
        let token = "ab".repeat(32);
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&format!("x=1; vigil_session={token}; y=2")).expect("header"),
        );
        assert_eq!(token_of(&headers), Some(token.as_str()));

        for bad in [
            "vigil_session=short",
            "vigil_session=",
            &format!("vigil_session={}", "zz".repeat(32)),
        ] {
            headers.insert(COOKIE, HeaderValue::from_str(bad).expect("header"));
            assert_eq!(token_of(&headers), None, "{bad}");
        }
        assert_eq!(token_of(&HeaderMap::new()), None);
    }

    #[test]
    fn the_cookie_is_locked_down_and_scoped_to_the_base_path() {
        let plain = WebConfig::new(Auth::Password, Some("http://10.0.0.2:5800"));
        let value = set_cookie(&plain, "tok").expect("cookie");
        let value = value.to_str().expect("text");
        assert!(
            value.starts_with("vigil_session=tok; Path=/; Max-Age=2592000;"),
            "{value}"
        );
        assert!(value.contains("HttpOnly") && value.contains("SameSite=Lax"));
        assert!(!value.contains("Secure"));

        let secure = WebConfig::new(Auth::Password, Some("https://example.com/vigil"));
        let value = set_cookie(&secure, "tok").expect("cookie");
        let value = value.to_str().expect("text");
        assert!(
            value.contains("Path=/vigil;") && value.ends_with("; Secure"),
            "{value}"
        );

        let cleared = clear_cookie(&secure).expect("cookie");
        assert!(
            cleared
                .to_str()
                .expect("text")
                .contains("vigil_session=; Path=/vigil; Max-Age=0")
        );
    }
}
