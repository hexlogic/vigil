//! Parsing notification URLs: what is accepted and how it is shown back.

mod common;

use common::{err, ok};
use vigil_notify::{Channel, HttpAuth, ParseError, SCHEMES, Secret, SmtpTls, WebhookFormat};

#[test]
fn ntfy_on_a_local_port() {
    let Channel::Ntfy(ntfy) = ok("ntfy://127.0.0.1:8080/topic") else {
        panic!("not ntfy");
    };
    assert_eq!(ntfy.base.as_str(), "http://127.0.0.1:8080/");
    assert_eq!(ntfy.topic.expose(), "topic");
    assert_eq!((ntfy.priority, ntfy.tags.len(), ntfy.auth), (None, 0, None));
}

#[test]
fn ntfys_with_options() {
    let Channel::Ntfy(ntfy) = ok("ntfys://ntfy.sh/alerts?priority=high&tags=warning, skull") else {
        panic!("not ntfy");
    };
    assert_eq!(ntfy.base.as_str(), "https://ntfy.sh/");
    assert_eq!(ntfy.priority, Some(4));
    assert_eq!(ntfy.tags, ["warning", "skull"]);
}

#[test]
fn ntfy_priorities_by_name_and_number() {
    for (text, expected) in [
        ("min", 1),
        ("2", 2),
        ("default", 3),
        ("HIGH", 4),
        ("urgent", 5),
    ] {
        let Channel::Ntfy(ntfy) = ok(&format!("ntfy://h/t?priority={text}")) else {
            panic!("not ntfy");
        };
        assert_eq!(ntfy.priority, Some(expected), "{text}");
    }
}

#[test]
fn ntfy_with_a_path_prefix_and_basic_auth() {
    let Channel::Ntfy(ntfy) = ok("ntfys://me:p%40ss@ntfy.example.com:8443/team/alerts") else {
        panic!("not ntfy");
    };
    assert_eq!(ntfy.base.as_str(), "https://ntfy.example.com:8443/team/");
    assert_eq!(ntfy.topic.expose(), "alerts");
    match ntfy.auth {
        Some(HttpAuth::Basic { user, password }) => {
            assert_eq!(user, "me");
            assert_eq!(password.expose(), "p@ss");
        }
        other => panic!("expected basic auth, got {other:?}"),
    }
}

#[test]
fn ntfy_with_a_bearer_token() {
    let Channel::Ntfy(ntfy) = ok("ntfy://h/t?token=tk_abc") else {
        panic!("not ntfy");
    };
    assert_eq!(ntfy.auth, Some(HttpAuth::Bearer(Secret::new("tk_abc"))));
}

#[test]
fn scheme_and_host_case_do_not_matter() {
    let Channel::Ntfy(ntfy) = ok("NTFY://Example.COM/topic") else {
        panic!("not ntfy");
    };
    assert_eq!(ntfy.base.as_str(), "http://example.com/");
}

#[test]
fn telegram_chats() {
    for (url, chat) in [
        ("tgram://123456:ABC-def_1/-100123456", "-100123456"),
        ("tgram://123456:ABC-def_1/987", "987"),
        ("tgram://123456:ABC-def_1/@my_channel", "@my_channel"),
        ("tgram://123456:ABC-def_1/%40my_channel/", "@my_channel"),
    ] {
        let Channel::Telegram(telegram) = ok(url) else {
            panic!("not telegram");
        };
        assert_eq!(telegram.token.expose(), "123456:ABC-def_1");
        assert_eq!(telegram.chat_id, chat);
    }
}

#[test]
fn discord_webhook_with_a_username() {
    let Channel::Discord(discord) = ok("discord://123456789/abc-DEF_1?username=Vigil%20Bot") else {
        panic!("not discord");
    };
    assert_eq!(discord.id, "123456789");
    assert_eq!(discord.token.expose(), "abc-DEF_1");
    assert_eq!(discord.username.as_deref(), Some("Vigil Bot"));
}

#[test]
fn slack_webhook() {
    let Channel::Slack(slack) = ok("slack://T000/B000/XXXX") else {
        panic!("not slack");
    };
    assert_eq!((slack.team.as_str(), slack.bot.as_str()), ("T000", "B000"));
    assert_eq!(slack.token.expose(), "XXXX");
}

#[test]
fn mailto_uses_starttls_on_587_by_default() {
    let Channel::Email(email) = ok("mailto://me%40example.com:pw@smtp.example.com?to=a@b.co")
    else {
        panic!("not email");
    };
    assert_eq!((email.tls, email.port), (SmtpTls::Starttls, 587));
    assert_eq!(email.host, "smtp.example.com");
    assert_eq!(email.from.email.to_string(), "me@example.com");
    assert_eq!(email.to.len(), 1);
    assert_eq!(email.to[0].email.to_string(), "a@b.co");
    let (user, password) = email.credentials.expect("credentials");
    assert_eq!((user.as_str(), password.expose()), ("me@example.com", "pw"));
}

#[test]
fn an_unencoded_at_sign_in_the_user_name_works() {
    let Channel::Email(email) = ok("mailto://me@example.com:pw@smtp.example.com") else {
        panic!("not email");
    };
    let (user, _) = email.credentials.expect("credentials");
    assert_eq!(user, "me@example.com");
    // The sender doubles as the recipient when no `to` is given.
    assert_eq!(email.to, std::slice::from_ref(&email.from));
}

#[test]
fn mailtos_uses_implicit_tls_on_465() {
    let Channel::Email(email) = ok("mailtos://smtp.example.com?from=v@example.com&to=a@b.co")
    else {
        panic!("not email");
    };
    assert_eq!((email.tls, email.port), (SmtpTls::Implicit, 465));
}

#[test]
fn mailto_without_tls_is_for_relays() {
    let Channel::Email(email) =
        ok("mailto://relay.lan?tls=none&from=Vigil <v@x.test>&to=a@b.co,c@d.co&name=Watcher")
    else {
        panic!("not email");
    };
    assert_eq!((email.tls, email.port), (SmtpTls::None, 25));
    assert_eq!(email.to.len(), 2);
    assert_eq!(email.from.name.as_deref(), Some("Watcher"));
    assert_eq!(email.from.email.to_string(), "v@x.test");
    assert!(email.credentials.is_none());
}

#[test]
fn a_password_without_tls_is_allowed_on_this_machine_only() {
    assert!(matches!(
        ok("mailto://u%40x.test:pw@127.0.0.1:2525?tls=none"),
        Channel::Email(_)
    ));
    assert!(matches!(
        ok("mailto://u%40x.test:pw@localhost?tls=none"),
        Channel::Email(_)
    ));
    let error = err("mailto://u%40x.test:pw@mail.example.com?tls=none").to_string();
    assert!(error.contains("without TLS"), "{error}");
}

#[test]
fn webhooks_keep_their_path_and_query() {
    let Channel::Webhook(hook) = ok("json://hooks.example.com/in/tok123?room=7") else {
        panic!("not a webhook");
    };
    assert_eq!(hook.format, WebhookFormat::Json);
    assert_eq!(
        hook.url.expose().as_str(),
        "http://hooks.example.com/in/tok123?room=7"
    );
    assert!(hook.auth.is_none());

    let Channel::Webhook(hook) = ok("forms://me:pw@hooks.example.com:8443") else {
        panic!("not a webhook");
    };
    assert_eq!(hook.format, WebhookFormat::Form);
    assert_eq!(
        hook.url.expose().as_str(),
        "https://hooks.example.com:8443/"
    );
    assert!(matches!(hook.auth, Some(HttpAuth::Basic { .. })));

    assert!(matches!(ok("jsons://h/x"), Channel::Webhook(w) if w.url.expose().scheme() == "https"));
    assert!(matches!(ok("form://h/x"), Channel::Webhook(w) if w.format == WebhookFormat::Form));
}

#[test]
fn gotify_with_and_without_a_prefix() {
    let Channel::Gotify(gotify) = ok("gotify://gotify.lan/Aabc.123?priority=8") else {
        panic!("not gotify");
    };
    assert_eq!(gotify.base.as_str(), "http://gotify.lan/");
    assert_eq!(gotify.token.expose(), "Aabc.123");
    assert_eq!(gotify.priority, Some(8));

    let Channel::Gotify(gotify) = ok("gotifys://example.com/push/TOKEN") else {
        panic!("not gotify");
    };
    assert_eq!(gotify.base.as_str(), "https://example.com/push/");
    assert_eq!(gotify.token.expose(), "TOKEN");
}

#[test]
fn every_listed_scheme_is_handled() {
    for scheme in SCHEMES {
        // Whatever else is wrong with this URL, the scheme must be recognised.
        let error = Channel::parse(&format!("{scheme}://")).err();
        assert!(
            !matches!(
                error,
                Some(ParseError::Unsupported { .. } | ParseError::NotAUrl)
            ),
            "{scheme}"
        );
    }
}

#[test]
fn redacted_urls_hide_secrets_and_keep_the_rest() {
    let cases: &[(&str, &str)] = &[
        ("ntfy://127.0.0.1:8080/topic", "ntfy://127.0.0.1:8080/***"),
        (
            "ntfys://me:pw@ntfy.sh/team/alerts?priority=high&tags=a,b",
            "ntfys://me:***@ntfy.sh/team/***?priority=4&tags=a,b",
        ),
        ("ntfy://h/t?token=abc", "ntfy://h/***?token=***"),
        ("tgram://123456:ABC/-100123", "tgram://***/-100123"),
        ("discord://123/tok", "discord://123/***"),
        ("slack://T000/B000/XXXX", "slack://T000/B000/***"),
        (
            "mailto://me%40x.test:pw@smtp.example.com?to=a@b.co",
            "mailto://me@x.test:***@smtp.example.com:587?to=a@b.co",
        ),
        (
            "mailtos://smtp.example.com:2465?from=v@x.test&to=a@b.co,c@d.co",
            "mailtos://smtp.example.com:2465?to=a@b.co,c@d.co",
        ),
        ("json://h/in/tok?k=1", "json://h/***"),
        ("jsons://u:p@h:8443/", "jsons://u:***@h:8443/"),
        ("forms://h", "forms://h/"),
        (
            "gotify://g.lan/TOKEN?priority=3",
            "gotify://g.lan/***?priority=3",
        ),
    ];
    for (url, expected) in cases {
        assert_eq!(ok(url).redacted(), *expected, "{url}");
    }
}

#[test]
fn kinds_are_named() {
    for (url, kind) in [
        ("ntfy://h/t", "ntfy"),
        ("tgram://1:a/2", "telegram"),
        ("discord://1/a", "discord"),
        ("slack://T/B/x", "slack"),
        ("mailto://h?from=a@b.co", "email"),
        ("json://h", "webhook"),
        ("gotify://h/t", "gotify"),
    ] {
        assert_eq!(ok(url).kind(), kind);
    }
}

#[test]
fn a_plus_in_an_option_is_a_plus() {
    let Channel::Email(email) =
        ok("mailto://smtp.example.com?from=vigil@example.com&to=me+vigil@gmail.com")
    else {
        panic!("not email");
    };
    assert_eq!(email.to[0].email.to_string(), "me+vigil@gmail.com");

    let Channel::Ntfy(ntfy) = ok("ntfy://h/t?token=ab+cd") else {
        panic!("not ntfy");
    };
    assert_eq!(ntfy.auth, Some(HttpAuth::Bearer(Secret::new("ab+cd"))));

    // A space is written %20, and a plus can also be written %2B.
    let Channel::Discord(discord) = ok("discord://1/tok?username=Vigil%20Bot+%2B") else {
        panic!("not discord");
    };
    assert_eq!(discord.username.as_deref(), Some("Vigil Bot++"));
}

#[test]
fn an_ipv6_mail_server_is_stored_without_brackets_and_shown_with_them() {
    let channel = ok("mailto://[::1]:2525?tls=none&from=a@b.co&to=d@e.co");
    let Channel::Email(email) = &channel else {
        panic!("not email");
    };
    assert_eq!((email.host.as_str(), email.port), ("::1", 2525));
    assert_eq!(channel.redacted(), "mailto://[::1]:2525?to=d@e.co&tls=none");
}
