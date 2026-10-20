//! Escaping page text for the markup each channel understands.
//!
//! Diff lines come from arbitrary web pages. Without escaping, `_` or `<` in a page makes
//! Telegram refuse the message, and `@everyone` or `<!channel>` would ping a whole server.

/// The markup a channel's message body is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Markup {
    /// Plain text: email, ntfy, Gotify and webhooks.
    Plain,
    /// Telegram's HTML subset.
    TelegramHtml,
    /// Discord's Markdown.
    DiscordMarkdown,
    /// Slack's `mrkdwn`.
    SlackMrkdwn,
}

impl Markup {
    /// The short name used in template names and the escape mode.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::TelegramHtml => "telegram",
            Self::DiscordMarkdown => "discord",
            Self::SlackMrkdwn => "slack",
        }
    }

    /// The markup whose [`name`](Self::name) is `name`.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        [
            Self::Plain,
            Self::TelegramHtml,
            Self::DiscordMarkdown,
            Self::SlackMrkdwn,
        ]
        .into_iter()
        .find(|markup| markup.name() == name)
    }
}

/// Characters that Discord's Markdown would act on. A backslash before punctuation is an escape
/// there and renders as the punctuation alone.
const DISCORD_SPECIAL: &str = "\\*_~`|><[]()#-";

/// Escape `text` so that it shows up literally in a message in `markup`.
pub fn escape(markup: Markup, text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match markup {
            Markup::Plain => out.push(c),
            Markup::TelegramHtml => match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                _ => out.push(c),
            },
            Markup::DiscordMarkdown => {
                if DISCORD_SPECIAL.contains(c) {
                    out.push('\\');
                }
                out.push(c);
            }
            Markup::SlackMrkdwn => match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                _ => out.push(c),
            },
        }
    }
    out
}

/// Put `text` in a preformatted block. The result is markup, not text to escape again.
pub fn code_block(markup: Markup, text: &str) -> String {
    // A diff ends in a newline; the closing fence supplies its own.
    let text = text.trim_end_matches('\n');
    match markup {
        Markup::Plain => text.to_owned(),
        Markup::TelegramHtml => format!("<pre>{}</pre>", escape(markup, text)),
        Markup::DiscordMarkdown => format!("```diff\n{}\n```", break_fences(text)),
        Markup::SlackMrkdwn => format!("```\n{}\n```", escape(markup, &break_fences(text))),
    }
}

/// `url` as a link in `markup`. The result is markup, not text to escape again.
pub fn link(markup: Markup, url: &str) -> String {
    match markup {
        Markup::Plain => url.to_owned(),
        // Telegram makes bare URLs clickable by itself.
        Markup::TelegramHtml => escape(markup, url),
        // Angle brackets keep Discord from reading `_` and `*` inside the URL as formatting.
        Markup::DiscordMarkdown => format!("<{}>", url.replace('>', "%3E").replace(' ', "%20")),
        Markup::SlackMrkdwn => format!("<{}>", escape(markup, url)),
    }
}

/// A clickable "Full diff" link to `url` in `markup`.
pub(crate) fn diff_link(markup: Markup, url: &str) -> String {
    match markup {
        Markup::TelegramHtml => {
            format!("<a href=\"{}\">Full diff</a>", escape(markup, url))
        }
        Markup::SlackMrkdwn => format!("<{}|Full diff>", escape(markup, url)),
        Markup::Plain | Markup::DiscordMarkdown => format!("Full diff: {}", link(markup, url)),
    }
}

/// Keep a run of three backticks in `text` from closing a code fence, by splitting it with a
/// zero-width space.
fn break_fences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = 0;
    for c in text.chars() {
        if c == '`' {
            if run == 2 {
                out.push('\u{200B}');
                run = 0;
            }
            run += 1;
        } else {
            run = 0;
        }
        out.push(c);
    }
    out
}
