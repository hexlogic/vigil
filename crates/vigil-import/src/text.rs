//! Small helpers every format uses.

/// An address as it may be shown in a report: no user name, password, query or fragment, since
/// those often hold tokens.
pub(crate) fn short_url(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let rest = rest.rsplit_once('@').map_or(rest, |(_, after)| after);
    if scheme.is_empty() {
        rest.to_owned()
    } else {
        format!("{scheme}://{rest}")
    }
}

/// The scheme of a notification address, if it has the shape of one, for saying which kind
/// was not carried over without repeating the address.
pub(crate) fn scheme_of(url: &str) -> Option<String> {
    let scheme = url.split_once("://")?.0;
    let shaped = !scheme.is_empty()
        && scheme.len() <= 20
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-');
    shaped.then(|| scheme.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::{scheme_of, short_url};

    #[test]
    fn only_a_scheme_shaped_prefix_is_named() {
        assert_eq!(
            scheme_of("TGRAM://bot:token/chat").as_deref(),
            Some("tgram")
        );
        assert_eq!(scheme_of("no scheme here"), None);
        assert_eq!(scheme_of("this-scheme-is-far-too-long://x"), None);
        assert_eq!(scheme_of("sp ace://x"), None);
    }

    #[test]
    fn a_shown_address_has_no_credentials_query_or_fragment() {
        assert_eq!(
            short_url("https://u:p@host.example/a/b?token=x#top"),
            "https://host.example/a/b"
        );
        assert_eq!(short_url("http://host.example"), "http://host.example");
        assert_eq!(short_url("not a url?x=1"), "not a url");
    }
}
