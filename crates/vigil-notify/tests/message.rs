//! Building messages: escaping, templates, titles and fallbacks.

use vigil_notify::{
    Channel, Event, Limit, Markup, Prepared, code_block, escape, link, prepare, validate_template,
};

fn channel(url: &str) -> Channel {
    match Channel::parse(url) {
        Ok(channel) => channel,
        Err(error) => panic!("{url}: {error}"),
    }
}

fn prep(url: &str, event: &Event, template: Option<&str>) -> Prepared {
    let channel = channel(url);
    prepare(channel.markup(), channel.limit(), event, template)
}

const PLAIN: &str = "ntfy://h/topic";
const TELEGRAM: &str = "tgram://1:a/2";
const DISCORD: &str = "discord://1/a";
const SLACK: &str = "slack://T/B/x";

#[test]
fn telegram_escapes_html() {
    assert_eq!(
        escape(Markup::TelegramHtml, "<script>a&b\"c"),
        "&lt;script&gt;a&amp;b&quot;c"
    );
}

#[test]
fn discord_escapes_markdown_but_leaves_mentions_to_allowed_mentions() {
    assert_eq!(
        escape(
            Markup::DiscordMarkdown,
            "**bold** _x_ @everyone <@1> `c` [a](b)"
        ),
        r"\*\*bold\*\* \_x\_ @everyone \<@1\> \`c\` \[a\]\(b\)"
    );
}

#[test]
fn slack_escapes_only_the_three_special_characters() {
    assert_eq!(
        escape(Markup::SlackMrkdwn, "<!channel> & <b>*x*"),
        "&lt;!channel&gt; &amp; &lt;b&gt;*x*"
    );
}

#[test]
fn plain_text_is_left_alone() {
    assert_eq!(escape(Markup::Plain, "<b> & *x*"), "<b> & *x*");
}

#[test]
fn code_blocks_cannot_be_closed_from_inside() {
    for markup in [Markup::DiscordMarkdown, Markup::SlackMrkdwn] {
        let out = code_block(markup, "a```b````c`");
        assert_eq!(out.matches("```").count(), 2, "{out:?}");
    }
    assert_eq!(
        code_block(Markup::TelegramHtml, "<a> & b"),
        "<pre>&lt;a&gt; &amp; b</pre>"
    );
    assert_eq!(code_block(Markup::Plain, "x"), "x");
}

#[test]
fn links_are_wrapped_for_each_service() {
    let url = "https://x.test/a_b?c=1&d=2";
    assert_eq!(link(Markup::Plain, url), url);
    assert_eq!(
        link(Markup::TelegramHtml, url),
        "https://x.test/a_b?c=1&amp;d=2"
    );
    assert_eq!(
        link(Markup::DiscordMarkdown, url),
        "<https://x.test/a_b?c=1&d=2>"
    );
    assert_eq!(
        link(Markup::SlackMrkdwn, url),
        "<https://x.test/a_b?c=1&amp;d=2>"
    );
}

#[test]
fn a_template_escapes_what_it_prints_but_not_its_own_text() {
    let mut event = Event::sample();
    event.watch_name = "a<b&c".into();
    let out = prep(TELEGRAM, &event, Some("<i>{{ watch_name }}</i>"));
    assert_eq!(out.body, "<i>a&lt;b&amp;c</i>");
    let out = prep(DISCORD, &event, Some("**{{ watch_name }}**"));
    assert_eq!(out.body, r"**a\<b&c**");
    let out = prep(PLAIN, &event, Some("<i>{{ watch_name }}</i>"));
    assert_eq!(out.body, "<i>a<b&c</i>");
}

#[test]
fn hostile_page_text_cannot_ping_or_inject() {
    let mut event = Event::sample();
    event.diff = "+<!channel> @everyone <script>x</script> [click](http://evil.test)".into();
    let slack = prep(SLACK, &event, None).body;
    assert!(
        slack.contains("&lt;!channel&gt;") && !slack.contains("<!channel>"),
        "{slack}"
    );
    let telegram = prep(TELEGRAM, &event, None).body;
    assert!(
        telegram.contains("&lt;script&gt;") && !telegram.contains("<script>"),
        "{telegram}"
    );
    let discord = prep(DISCORD, &event, None).body;
    // Inside the code fence nothing is parsed; the text itself is not rewritten.
    assert!(discord.contains("```diff\n"), "{discord}");
}

#[test]
fn join_escapes_each_item_once() {
    let mut event = Event::sample();
    event.triggered = vec!["<a>".into(), "b&c".into()];
    let out = prep(
        TELEGRAM,
        &event,
        Some(r#"{{ triggered_text | join(", ") }}"#),
    );
    assert_eq!(out.body, "&lt;a&gt;, b&amp;c");
    let out = prep(PLAIN, &event, Some(r#"{{ triggered_text | join(", ") }}"#));
    assert_eq!(out.body, "<a>, b&c");
}

#[test]
fn the_title_comes_from_the_template_or_defaults_to_the_watch_name() {
    let event = Event::sample();
    let out = prep(
        PLAIN,
        &event,
        Some(r#"{% set title = "Hi " ~ watch_name %}the body"#),
    );
    assert_eq!(
        (out.title.as_str(), out.body.as_str()),
        ("Hi Example product page", "the body")
    );
    let out = prep(PLAIN, &event, Some("just a body"));
    assert_eq!(out.title, "Example product page changed");
}

#[test]
fn titles_are_one_short_line() {
    let event = Event::sample();
    let out = prep(PLAIN, &event, Some(r#"{% set title = "first\nsecond" %}x"#));
    assert_eq!(out.title, "first");
    let mut long = Event::sample();
    long.watch_name = "n".repeat(1000);
    assert!(prep(PLAIN, &long, None).title.chars().count() <= 200);
}

#[test]
fn every_documented_variable_is_available() {
    let template = "{{ watch_id }}|{{ watch_name }}|{{ watch_url }}|{{ diff_added }}|\
                    {{ diff_removed }}|{{ diff_full | length }}|{{ current_snapshot | length }}|\
                    {{ triggered_text | join(',') }}|{{ checked_at }}|{{ vigil_base_url }}|\
                    {{ diff_url | length }}|{{ truncated }}|{{ is_test }}";
    let event = Event::sample();
    let out = prep(PLAIN, &event, Some(template));
    assert_eq!(
        out.body,
        format!(
            "0192a3b4-c5d6-7e8f-9012-3456789abcde|Example product page|\
             https://example.com/products/widget|Widget: $12|Widget: $10|{}|{}|Widget: $12|\
             2026-01-01T00:00:00Z|https://vigil.example.com|{}|False|False",
            event.diff.len(),
            event.snapshot.len(),
            event.diff_url.len()
        )
    );
}

#[test]
fn bad_templates_are_caught_when_saved() {
    assert!(validate_template("{{ watch_name }} changed").is_ok());
    for bad in [
        "{{ wach_name }}",
        "{{ watch_name | nope }}",
        "{% if %}",
        "{{ watch_name | safe }}",
        "{{ watch_name",
        "{% for i in range(100000) %}{% for j in range(100000) %}x{% endfor %}{% endfor %}",
    ] {
        assert!(validate_template(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_template_that_fails_when_sending_falls_back_to_the_built_in_one() {
    let event = Event::sample();
    let builtin = prep(PLAIN, &event, None);
    assert_eq!(builtin.template_note, None);
    let broken = prep(PLAIN, &event, Some("{{ missing_variable }}"));
    assert_eq!(broken.body, builtin.body);
    let note = broken.template_note.expect("a note");
    assert!(
        note.contains("custom template") && note.contains("built-in"),
        "{note}"
    );
}

#[test]
fn built_in_messages_have_a_stable_shape() {
    let event = Event::sample();
    for (name, url) in [
        ("plain", PLAIN),
        ("telegram", TELEGRAM),
        ("discord", DISCORD),
        ("slack", SLACK),
    ] {
        let out = prep(url, &event, None);
        insta::assert_snapshot!(name, format!("title: {}\n---\n{}", out.title, out.body));
    }
}

#[test]
fn a_change_without_a_diff_still_makes_a_message() {
    let mut event = Event::sample();
    event.added.clear();
    event.removed.clear();
    event.diff.clear();
    event.triggered.clear();
    event.diff_url.clear();
    let out = prep(TELEGRAM, &event, None);
    assert_eq!(
        out.body,
        "<b>Example product page</b> changed\nhttps://example.com/products/widget"
    );
    assert!(!out.truncated);
}

#[test]
fn limits_count_the_unit_the_service_counts() {
    assert!(Limit::Chars(3).fits("абв"));
    assert!(!Limit::Bytes(3).fits("абв"));
    assert_eq!(Limit::Bytes(5).measure("абв"), 6);
    assert_eq!(channel(PLAIN).limit(), Limit::Bytes(4096));
    assert_eq!(channel(TELEGRAM).limit(), Limit::Chars(4096));
    assert_eq!(channel(DISCORD).limit(), Limit::Chars(2000));
}
