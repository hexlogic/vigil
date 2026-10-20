//! Content hashing for change detection.

use std::fmt;

/// A blake3 digest of a snapshot's processed text.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// Hash `text`.
    pub fn of(text: &str) -> Self {
        Self(*blake3::hash(text.as_bytes()).as_bytes())
    }

    /// The raw 32 digest bytes, as stored in the database.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Rebuild a hash from stored bytes. Returns `None` unless `bytes` is exactly 32 long.
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        <[u8; 32]>::try_from(bytes).ok().map(Self)
    }

    /// Lowercase hex form of the digest.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({})", self.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_text_hashes_equal_and_different_text_differs() {
        assert_eq!(ContentHash::of("a"), ContentHash::of("a"));
        assert_ne!(ContentHash::of("a"), ContentHash::of("b"));
    }

    #[test]
    fn round_trips_through_bytes() {
        let hash = ContentHash::of("hello");
        assert_eq!(ContentHash::from_slice(hash.as_bytes()), Some(hash));
    }

    #[test]
    fn from_slice_rejects_wrong_length() {
        assert_eq!(ContentHash::from_slice(&[0; 31]), None);
        assert_eq!(ContentHash::from_slice(&[0; 33]), None);
    }

    #[test]
    fn hex_is_64_lowercase_chars() {
        let hex = ContentHash::of("hello").to_hex();
        assert_eq!(hex.len(), 64);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
    }
}
