//! API tokens: made here, kept by their hash.

use crate::auth::session;

/// What a token starts with, so one is easy to recognise in a config file or a leak scan.
const PREFIX: &str = "vigil_";

/// Characters of a token that are shown to tell tokens apart: the prefix and four more.
const SHOWN: usize = PREFIX.len() + 4;

/// A token that was just made. It is shown to the person once; only its hash and a short
/// beginning are kept.
#[derive(Debug, Clone)]
pub struct GeneratedToken {
    /// The token to send in the `x-api-key` header.
    pub token: String,
    /// What the database knows it by.
    pub hash: [u8; 32],
    /// The first characters of the token, to tell tokens apart in a list.
    pub prefix: String,
}

/// Make a new random token: `vigil_` and 256 random bits in hex.
pub fn generate_token() -> Result<GeneratedToken, getrandom::Error> {
    let token = format!("{PREFIX}{}", session::new_token()?);
    Ok(GeneratedToken {
        hash: hash_token(&token),
        prefix: token[..SHOWN].to_owned(),
        token,
    })
}

/// What the database knows a token by.
pub(crate) fn hash_token(token: &str) -> [u8; 32] {
    session::hash_token(token)
}

/// Whether text has the shape of a token, so anything else is turned away without a lookup.
pub(crate) fn looks_like_token(text: &str) -> bool {
    text.strip_prefix(PREFIX)
        .is_some_and(|rest| rest.len() == 64 && rest.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::{generate_token, hash_token, looks_like_token};

    #[test]
    fn tokens_are_random_recognisable_and_hashed() -> Result<(), getrandom::Error> {
        let a = generate_token()?;
        let b = generate_token()?;
        assert_ne!(a.token, b.token);
        assert!(looks_like_token(&a.token), "{}", a.token);
        assert_eq!(a.token.len(), "vigil_".len() + 64);
        assert_eq!(a.hash, hash_token(&a.token));
        assert_eq!(a.prefix, a.token[..10]);
        assert!(a.token.starts_with(&a.prefix));
        Ok(())
    }

    #[test]
    fn only_the_right_shape_is_looked_up() {
        assert!(!looks_like_token(""));
        assert!(!looks_like_token("vigil_"));
        assert!(!looks_like_token(&format!("other_{}", "a".repeat(64))));
        assert!(!looks_like_token(&format!("vigil_{}", "g".repeat(64))));
        assert!(!looks_like_token(&format!("vigil_{}", "a".repeat(63))));
        assert!(looks_like_token(&format!("vigil_{}", "A".repeat(64))));
    }
}
