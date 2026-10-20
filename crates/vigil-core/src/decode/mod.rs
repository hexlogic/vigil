//! Charset decoding of fetched bodies.

mod prescan;

use encoding_rs::{Encoding, REPLACEMENT, UTF_8};
use prescan::prescan;

/// Decode `bytes` to text.
///
/// The charset is the first that applies of: a byte-order mark in the body, the `charset`
/// parameter of the `Content-Type` header, and a charset declared in the first kilobyte of an
/// HTML or XML body (`<meta charset>`, `<meta http-equiv="content-type">`, or the encoding of
/// the `<?xml?>` declaration). It defaults to UTF-8 when none is given or the label is unknown.
/// Malformed sequences become U+FFFD instead of failing.
pub fn decode(bytes: &[u8], content_type: Option<&str>) -> String {
    let encoding = content_type
        .and_then(charset_of)
        .or_else(|| prescan(bytes, content_type))
        .unwrap_or(UTF_8);
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
    use encoding_rs::WINDOWS_1251;

    use super::decode;

    const CYRILLIC: &str = "Привіт, світе";

    fn cp1251(text: &str) -> Vec<u8> {
        WINDOWS_1251.encode(text).0.into_owned()
    }

    fn page(head: &str, body: &str) -> Vec<u8> {
        let mut bytes = head.as_bytes().to_vec();
        bytes.extend(cp1251(body));
        bytes
    }

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

    #[test]
    fn meta_charset_is_used_without_a_header() {
        let bytes = page(
            "<html><head><meta charset=\"windows-1251\"></head><body>",
            CYRILLIC,
        );
        assert!(decode(&bytes, Some("text/html")).ends_with(CYRILLIC));
        assert!(decode(&bytes, None).ends_with(CYRILLIC));
    }

    #[test]
    fn meta_charset_is_case_insensitive_and_may_be_unquoted() {
        let bytes = page("<HTML><META CHARSET=Windows-1251><body>", CYRILLIC);
        assert!(decode(&bytes, Some("text/html")).ends_with(CYRILLIC));
    }

    #[test]
    fn http_equiv_content_type_is_used() {
        let head = "<meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1251\">";
        let bytes = page(head, CYRILLIC);
        assert!(decode(&bytes, Some("text/html")).ends_with(CYRILLIC));
    }

    #[test]
    fn other_meta_tags_are_skipped() {
        let head =
            "<metadata x=1><meta name=\"viewport\" content=\"width=1\"><meta charset=windows-1251>";
        let bytes = page(head, CYRILLIC);
        assert!(decode(&bytes, Some("text/html")).ends_with(CYRILLIC));
    }

    #[test]
    fn header_beats_meta() {
        let bytes = page("<meta charset=windows-1251>", CYRILLIC);
        let decoded = decode(&bytes, Some("text/html; charset=utf-8"));
        assert!(!decoded.ends_with(CYRILLIC));
    }

    #[test]
    fn an_unknown_header_charset_leaves_room_for_meta() {
        let bytes = page("<meta charset=windows-1251>", CYRILLIC);
        let decoded = decode(&bytes, Some("text/html; charset=nonsense"));
        assert!(decoded.ends_with(CYRILLIC));
    }

    #[test]
    fn bom_beats_meta() {
        let mut bytes = vec![0xef, 0xbb, 0xbf];
        bytes.extend_from_slice(b"<meta charset=windows-1251>");
        bytes.extend_from_slice(CYRILLIC.as_bytes());
        assert!(decode(&bytes, Some("text/html")).ends_with(CYRILLIC));
    }

    #[test]
    fn xml_declaration_encoding_is_used() {
        let bytes = page(
            "<?xml version=\"1.0\" encoding=\"windows-1251\"?><rss>",
            CYRILLIC,
        );
        assert!(decode(&bytes, Some("application/rss+xml")).ends_with(CYRILLIC));
        assert!(decode(&bytes, Some("text/xml")).ends_with(CYRILLIC));
        assert!(decode(&bytes, None).ends_with(CYRILLIC));
    }

    #[test]
    fn meta_tags_are_ignored_in_non_html_xml() {
        let bytes = page("<rss><meta charset=windows-1251>", CYRILLIC);
        assert!(!decode(&bytes, Some("application/rss+xml")).ends_with(CYRILLIC));
    }

    #[test]
    fn declared_utf16_means_utf8() {
        let mut bytes = b"<meta charset=utf-16>".to_vec();
        bytes.extend_from_slice(CYRILLIC.as_bytes());
        assert!(decode(&bytes, Some("text/html")).ends_with(CYRILLIC));
    }

    #[test]
    fn declared_x_user_defined_means_windows_1252() {
        let bytes = b"<meta charset=x-user-defined>caf\xe9";
        assert!(decode(bytes, Some("text/html")).ends_with("café"));
    }

    #[test]
    fn a_declaration_beyond_the_first_kilobyte_is_ignored() {
        let padding = " ".repeat(1100);
        let bytes = page(&format!("{padding}<meta charset=windows-1251>"), CYRILLIC);
        assert!(!decode(&bytes, Some("text/html")).ends_with(CYRILLIC));
    }

    #[test]
    fn a_tag_cut_off_by_the_window_is_ignored() {
        let padding = " ".repeat(1010);
        let bytes = page(&format!("{padding}<meta charset=windows-1251>"), CYRILLIC);
        assert!(!decode(&bytes, Some("text/html")).ends_with(CYRILLIC));
    }

    #[test]
    fn json_and_plain_text_are_not_prescanned() {
        let bytes = page("{\"a\": \"<meta charset=windows-1251>\"} ", CYRILLIC);
        for content_type in ["application/json", "text/plain"] {
            assert!(!decode(&bytes, Some(content_type)).ends_with(CYRILLIC));
        }
    }

    #[test]
    fn the_replacement_encoding_is_not_used_when_declared_in_the_page() {
        assert_eq!(
            decode(b"<meta charset=iso-2022-kr>abc", Some("text/html")),
            "<meta charset=iso-2022-kr>abc"
        );
    }

    #[test]
    fn malformed_markup_does_not_panic() {
        for text in [
            "<meta",
            "<meta ",
            "<meta charset",
            "<meta charset=",
            "<meta charset='",
            "<meta =",
            "<meta ===>",
            "<meta / / />",
            "<?xml",
            "<?xml encoding",
            "<?xml encoding=\"",
            "<?xml encoding=\"x\" ?",
            "<meta http-equiv=content-type content=\"charset\">",
            "<meta http-equiv=content-type content=\"charset=\">",
        ] {
            let _ = decode(text.as_bytes(), Some("text/html"));
            let _ = decode(text.as_bytes(), None);
        }
    }
}
