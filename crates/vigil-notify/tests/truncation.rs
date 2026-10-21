//! Messages that are too long for their channel.

use vigil_notify::{Channel, Event, Limit, Prepared, prepare};

fn prep(url: &str, event: &Event, template: Option<&str>) -> (Channel, Prepared) {
    let channel = match Channel::parse(url) {
        Ok(channel) => channel,
        Err(error) => panic!("{url}: {error}"),
    };
    let prepared = prepare(channel.markup(), channel.limit(), event, template);
    (channel, prepared)
}

/// A page change with a lot of Ukrainian text, emoji and characters that every markup escapes.
fn big_event() -> Event {
    let line = "Привіт, світе! Ціна змінилась 🙂 <b>&</b> _*[x]_ `y`";
    let lines: Vec<String> = (0..3000).map(|i| format!("{i}: {line}")).collect();
    let mut event = Event::sample();
    event.removed = lines.iter().take(500).cloned().collect();
    event.diff = lines
        .iter()
        .map(|l| format!("+{l}"))
        .collect::<Vec<_>>()
        .join("\n");
    event.snapshot = lines.join("\n");
    event.triggered = lines.iter().take(60).cloned().collect();
    event.added = lines;
    event
}

const URLS: [&str; 7] = [
    "ntfy://h/topic",
    "tgram://1:a/2",
    "discord://1/a",
    "slack://T/B/x",
    "gotify://h/TOKEN",
    "mailto://h?from=a@b.co",
    "json://h/x",
];

#[test]
fn every_channel_stays_within_its_limit() {
    let event = big_event();
    for url in URLS {
        let (channel, out) = prep(url, &event, None);
        assert!(
            channel.limit().fits(&out.body),
            "{url}: {} units",
            channel.limit().measure(&out.body)
        );
        assert!(out.truncated, "{url}");
        assert!(out.body.contains("truncated"), "{url}");
        assert!(
            out.body.contains(&event.diff_url) || out.body.contains("&amp;to=b"),
            "{url}"
        );
    }
}

#[test]
fn ntfy_is_cut_by_bytes_not_characters() {
    let (_, out) = prep("ntfy://h/topic", &big_event(), None);
    assert!(out.body.len() <= 4096, "{}", out.body.len());
    assert!(
        out.body.len() > 3000,
        "should use most of the room: {}",
        out.body.len()
    );
}

#[test]
fn telegram_stays_well_formed_when_cut() {
    let (_, out) = prep("tgram://1:a/2", &big_event(), None);
    assert_eq!(
        out.body.matches("<pre>").count(),
        out.body.matches("</pre>").count()
    );
    assert_eq!(
        out.body.matches("<b>").count(),
        out.body.matches("</b>").count()
    );
    // Every ampersand starts one of the four entities Telegram's HTML accepts.
    for (i, _) in out.body.match_indices('&') {
        let rest = &out.body[i..];
        assert!(
            ["&amp;", "&lt;", "&gt;", "&quot;"]
                .iter()
                .any(|e| rest.starts_with(e)),
            "stray ampersand before {:?}",
            &rest[..rest.len().min(20)]
        );
    }
}

#[test]
fn discord_keeps_its_code_fence_closed() {
    let (_, out) = prep("discord://1/a", &big_event(), None);
    assert_eq!(
        out.body.matches("```").count() % 2,
        0,
        "{}",
        &out.body[out.body.len() - 200..]
    );
}

#[test]
fn the_notice_links_to_the_diff_in_each_markup() {
    let event = big_event();
    let tail = |url: &str| {
        let (_, out) = prep(url, &event, None);
        out.body
            .rsplit("… truncated.")
            .next()
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(
        tail("ntfy://h/t"),
        format!(" Full diff: {}", event.diff_url)
    );
    assert_eq!(
        tail("discord://1/a"),
        format!(" Full diff: <{}>", event.diff_url)
    );
    assert_eq!(
        tail("tgram://1:a/2"),
        format!(
            " <a href=\"{}\">Full diff</a>",
            event.diff_url.replace('&', "&amp;")
        )
    );
    assert_eq!(
        tail("slack://T/B/x"),
        format!(" <{}|Full diff>", event.diff_url.replace('&', "&amp;"))
    );
}

#[test]
fn without_a_base_url_the_notice_has_no_link() {
    let mut event = big_event();
    event.diff_url.clear();
    event.base_url.clear();
    let (_, out) = prep("ntfy://h/t", &event, None);
    assert!(
        out.body.ends_with("… truncated."),
        "{}",
        &out.body[out.body.len() - 60..]
    );
}

#[test]
fn only_the_values_a_template_prints_are_cut() {
    // The template prints only the added lines; the huge snapshot and diff must not eat the room.
    let event = big_event();
    let (channel, out) = prep("discord://1/a", &event, Some("{{ diff_added }}"));
    assert!(channel.limit().fits(&out.body));
    assert!(
        out.body.chars().count() > 1500,
        "{}",
        out.body.chars().count()
    );
}

#[test]
fn long_values_share_the_room_fairly() {
    let mut event = Event::sample();
    event.added = vec!["added line".into(); 400];
    event.removed = vec!["removed line".into(); 400];
    let (_, out) = prep(
        "ntfy://h/t",
        &event,
        Some("{{ diff_added }}\n==\n{{ diff_removed }}"),
    );
    let (added, removed) = out.body.split_once("\n==\n").expect("both parts");
    assert!(added.contains("added line") && removed.contains("removed line"));
    assert!(
        added.len() > 1000 && removed.len() > 1000,
        "{} / {}",
        added.len(),
        removed.len()
    );
}

#[test]
fn a_template_that_cannot_be_shortened_falls_back_to_a_minimal_message() {
    // The template itself is longer than the limit, whatever the values.
    let template = "x".repeat(5000);
    let (channel, out) = prep("discord://1/a", &Event::sample(), Some(&template));
    assert!(channel.limit().fits(&out.body));
    assert!(out.truncated);
    assert!(out.body.contains("Example product page changed"));
}

#[test]
fn a_huge_name_and_url_still_fit() {
    let mut event = big_event();
    event.watch_name = "н".repeat(5000);
    event.watch_url = format!("https://example.com/{}", "a&".repeat(5000));
    for url in URLS {
        let (channel, out) = prep(url, &event, Some(&"y".repeat(9000)));
        assert!(channel.limit().fits(&out.body), "{url}");
        assert!(Limit::Chars(200).fits(&out.title), "{url}");
    }
}

#[test]
fn a_template_that_indexes_a_short_list_survives_truncation() {
    // `triggered_text[0]` and `lines | first` exist for the whole event; the cut-down variants
    // used to fit the message must not turn that into an error.
    let mut event = big_event();
    event.triggered = vec!["Widget".into()];
    for template in [
        "{{ triggered_text[0] }}\n{{ diff_full | code }}",
        "{{ diff_added | lines | first }}\n{{ diff_full | code }}",
    ] {
        let (channel, out) = prep("discord://1/a", &event, Some(template));
        assert_eq!(out.template_note, None, "{template}");
        assert!(
            out.truncated && channel.limit().fits(&out.body),
            "{template}"
        );
        assert!(
            out.body.starts_with("Widget") || out.body.starts_with("0: "),
            "{}",
            &out.body[..40]
        );
    }
}
