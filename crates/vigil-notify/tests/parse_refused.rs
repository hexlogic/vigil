//! Refused notification URLs, and that no secret leaks from errors or `Debug` output.

mod common;

use common::{err, ok};
use vigil_notify::{Channel, ParseError, Secret};

#[test]
fn unknown_schemes_say_not_supported_yet() {
    for url in [
        "matrix://user:pw@matrix.org/!room:matrix.org",
        "https://example.com/hook",
        "pover://u/t",
    ] {
        let error = err(url);
        assert!(matches!(error, ParseError::Unsupported { .. }), "{url}");
        let text = error.to_string();
        assert!(text.contains("not supported yet"), "{text}");
        assert!(text.contains("ntfy") && text.contains("gotifys"), "{text}");
    }
}

#[test]
fn text_without_a_scheme_is_not_a_url() {
    for text in [
        "",
        "   ",
        "just words",
        "ntfy:/host/topic",
        "no scheme.example/x",
        "a b://x",
    ] {
        assert_eq!(err(text), ParseError::NotAUrl, "{text:?}");
    }
}

#[test]
fn refused_urls_say_why() {
    let cases: &[(&str, &str)] = &[
        // ntfy
        ("ntfy://host", "needs a topic"),
        ("ntfy:///topic", "host"),
        ("ntfy://host/bad%20topic", "topic must be"),
        ("ntfy://host/topic?priority=9", "priority="),
        ("ntfy://host/topic?colour=red", "option it does not know"),
        ("ntfy://host/topic?priority=1&priority=2", "more than once"),
        ("ntfy://u:p@host/topic?token=x", "not both"),
        ("ntfy://u@host/topic", "user name and a password"),
        ("ntfy://host/topic?token=", "token= is empty"),
        ("ntfy://host:99999/topic", "not a valid URL"),
        ("ntfy://host/topic#frag", "#fragment"),
        // telegram
        ("tgram://nocolon/123", "bot token"),
        ("tgram://123:abc", "expected tgram://"),
        ("tgram://123:abc/1/2", "expected tgram://"),
        ("tgram://123:abc/chat", "chat id"),
        ("tgram://123:abc/1?silent=1", "takes no options"),
        // discord
        ("discord://abc/tok", "webhook id"),
        ("discord://123", "expected discord://"),
        ("discord://123/tok?foo=1", "option it does not know"),
        ("discord://123/to!k", "webhook token"),
        // slack
        ("slack://T000/B000", "expected slack://"),
        ("slack://T000/B000/x!y", "letters and digits"),
        ("slack://T000/B000/XXXX?a=1", "takes no options"),
        // email
        ("mailto://smtp.example.com", "from="),
        ("mailto://host?from=nope", "valid email"),
        ("mailto://host?from=a@b.co&to=nope", "valid email"),
        ("mailto://host?from=a@b.co&tls=ssl", "starttls or none"),
        ("mailtos://host?from=a@b.co&tls=none", "always uses TLS"),
        ("mailto://host/path?from=a@b.co", "takes no path"),
        (
            "mailto://host?from=a@b.co&port=25",
            "option it does not know",
        ),
        // webhooks
        ("json://", "host"),
        ("json://u@host/x", "user name and a password"),
        // gotify
        ("gotify://host", "application token"),
        ("gotify://u:p@host/tok", "no user:password@"),
        ("gotify://host/tok?priority=11", "0 to 10"),
        ("gotify://host/to%20k", "token must be"),
    ];
    for (url, expected) in cases {
        let text = err(url).to_string();
        assert!(text.contains(expected), "{url}: {text}");
    }
}

/// URLs with a recognisable secret in each place one can go.
const SECRET_URLS: &[(&str, &str)] = &[
    (
        "ntfys://alice:S3CRETPW@ntfy.example.com/S3CRETTOPIC",
        "S3CRET",
    ),
    ("ntfy://ntfy.example.com/topic?token=S3CRETTOK", "S3CRET"),
    ("tgram://123456:S3CRETBOT/-100123", "S3CRET"),
    ("discord://123456/S3CRETHOOK?username=Bot", "S3CRET"),
    ("slack://T000/B000/S3CRETSLACK", "S3CRET"),
    (
        "mailto://me%40x.test:S3CRETSMTP@smtp.example.com?to=a@b.co",
        "S3CRET",
    ),
    (
        "json://alice:S3CRETPW@hooks.example.com/S3CRETPATH?key=S3CRETQ",
        "S3CRET",
    ),
    ("forms://hooks.example.com/in?key=S3CRETQ", "S3CRET"),
    ("gotifys://gotify.example.com/S3CRETGOT", "S3CRET"),
];

#[test]
fn secrets_never_appear_in_debug_or_redacted_output() {
    for (url, marker) in SECRET_URLS {
        let channel = ok(url);
        for shown in [
            format!("{channel:?}"),
            format!("{channel:#?}"),
            channel.redacted(),
        ] {
            assert!(!shown.contains(marker), "{url} leaked in {shown}");
        }
    }
}

#[test]
fn errors_never_repeat_the_url() {
    for (url, marker) in SECRET_URLS {
        let joiner = if url.contains('?') { '&' } else { '?' };
        for broken in [
            format!("{url}{joiner}{marker}=1"),
            format!("{url}{joiner}{marker}"),
            format!("{url}#{marker}"),
        ] {
            if let Err(error) = Channel::parse(&broken) {
                for shown in [error.to_string(), format!("{error:?}")] {
                    assert!(!shown.contains(marker), "{broken} leaked in {shown}");
                }
            }
        }
    }
    // A scheme that is not supported is shown; the rest of the URL is not.
    let error = err("matrix://user:S3CRETPW@matrix.org/room").to_string();
    assert!(
        error.contains("matrix://") && !error.contains("S3CRET"),
        "{error}"
    );
}

#[test]
fn secret_and_secret_url_print_as_stars() {
    let secret = Secret::new("hunter2");
    assert_eq!(format!("{secret} {secret:?}"), "*** Secret(***)");
    assert_eq!(secret.expose(), "hunter2");
}

#[test]
fn secrets_over_plain_http_are_flagged_unless_local() {
    for (url, flagged) in [
        ("ntfy://me:pw@ntfy.lan/t", true),
        ("ntfy://ntfy.lan/t?token=x", true),
        // The topic is the secret of a public ntfy server.
        ("ntfy://ntfy.lan/t", true),
        ("ntfy://ntfy.sh/my-secret-topic", true),
        ("ntfys://me:pw@ntfy.sh/t", false),
        ("ntfy://me:pw@127.0.0.1:8080/t", false),
        ("ntfy://me:pw@localhost/t", false),
        ("json://me:pw@hooks.lan/x", true),
        // The token of a webhook is usually in its path or query.
        ("json://hooks.lan/x", true),
        ("json://hooks.lan/?key=1", true),
        ("json://hooks.lan/", false),
        ("jsons://hooks.lan/x", false),
        ("gotify://gotify.lan/TOKEN", true),
        ("gotify://[::1]/TOKEN", false),
        ("gotifys://gotify.lan/TOKEN", false),
        ("tgram://1:a/2", false),
        ("mailto://smtp.lan?from=a@b.co&tls=none", false),
    ] {
        assert_eq!(ok(url).sends_secrets_in_cleartext(), flagged, "{url}");
    }
}
