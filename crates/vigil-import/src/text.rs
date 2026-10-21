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

#[cfg(test)]
mod tests {
    use super::short_url;

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
