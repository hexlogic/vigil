//! Charset decoding of fetched bodies.

use encoding_rs::{Encoding, REPLACEMENT, UTF_8};

/// Decode `bytes` to text.
///
/// The charset comes from the `Content-Type` header and defaults to UTF-8 when it is missing
/// or unknown. A byte-order mark in the body takes precedence over the header. Malformed
/// sequences become U+FFFD instead of failing. Sniffing `<meta charset>` is not done yet.
pub fn decode(bytes: &[u8], content_type: Option<&str>) -> String {
    let encoding = content_type.and_then(charset_of).unwrap_or(UTF_8);
    let (text, _used, _had_errors) = encoding.decode(bytes);
    text.into_owned()
}

fn charset_of(content_type: &str) -> Option<&'static Encoding> {
    content_type
        .split(';')
        .skip(1)
        .filter_map(|param| param.split_once('='))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case("charset"))
        .and_then(|(_, value)| Encoding::for_label(value.trim().trim_matches('"').as_bytes()))
        // Labels such as iso-2022-kr map to the REPLACEMENT encoding, which decodes any input
        // to a single U+FFFD. Using it would make every such page look identical forever.
        .filter(|encoding| *encoding != REPLACEMENT)
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[test]
    fn defaults_to_utf8() {
        assert_eq!(decode("héllo".as_bytes(), None), "héllo");
        assert_eq!(decode("héllo".as_bytes(), Some("text/html")), "héllo");
    }

    #[test]
    fn honours_header_charset() {
        let latin1 = [0x68, 0xe9, 0x6c, 0x6c, 0x6f];
        assert_eq!(
            decode(&latin1, Some("text/html; charset=ISO-8859-1")),
            "héllo"
        );
        assert_eq!(
            decode(&latin1, Some("text/html; Charset=\"latin1\"")),
            "héllo"
        );
    }

    #[test]
    fn unknown_charset_falls_back_to_utf8() {
        assert_eq!(decode(b"abc", Some("text/html; charset=nonsense")), "abc");
    }

    #[test]
    fn the_replacement_encoding_is_not_used() {
        for label in ["iso-2022-kr", "iso-2022-cn", "hz-gb-2312"] {
            let header = format!("text/html; charset={label}");
            assert_eq!(decode(b"abc", Some(&header)), "abc", "{label}");
            assert_ne!(
                decode(b"one", Some(&header)),
                decode(b"two", Some(&header)),
                "{label}"
            );
        }
    }

    #[test]
    fn bom_overrides_header() {
        let mut bytes = vec![0xef, 0xbb, 0xbf];
        bytes.extend_from_slice("héllo".as_bytes());
        assert_eq!(
            decode(&bytes, Some("text/html; charset=iso-8859-1")),
            "héllo"
        );
    }

    #[test]
    fn invalid_utf8_is_replaced_not_rejected() {
        assert_eq!(decode(&[0x61, 0xff, 0x62], None), "a\u{fffd}b");
    }
}
