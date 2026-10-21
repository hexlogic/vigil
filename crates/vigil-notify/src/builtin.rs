//! The message templates used when neither the watch nor the target has one.

use crate::escape::Markup;

/// Plain text: the title is sent separately by channels that have a title.
const PLAIN: &str = r#"{{ watch_url }}
{%- if diff_added %}

Added:
{{ diff_added }}
{%- endif %}
{%- if diff_removed %}

Removed:
{{ diff_removed }}
{%- endif %}
{%- if triggered_text %}

Matched: {{ triggered_text | join(", ") }}
{%- endif %}
{%- if diff_url %}

Diff: {{ diff_url }}
{%- endif %}"#;

const TELEGRAM: &str = r#"<b>{{ watch_name }}</b>{% if not is_test %} changed{% endif %}
{{ watch_url | link }}
{%- if triggered_text %}

Matched: {{ triggered_text | join(", ") }}
{%- endif %}
{%- if diff_full %}

{{ diff_full | code }}
{%- endif %}
{%- if diff_url %}

<a href="{{ diff_url }}">Full diff</a>
{%- endif %}"#;

const DISCORD: &str = r#"**{{ watch_name }}**{% if not is_test %} changed{% endif %}
{{ watch_url | link }}
{%- if triggered_text %}

Matched: {{ triggered_text | join(", ") }}
{%- endif %}
{%- if diff_full %}

{{ diff_full | code }}
{%- endif %}
{%- if diff_url %}

{{ diff_url | link }}
{%- endif %}"#;

const SLACK: &str = r#"*{{ watch_name }}*{% if not is_test %} changed{% endif %}
{{ watch_url | link }}
{%- if triggered_text %}

Matched: {{ triggered_text | join(", ") }}
{%- endif %}
{%- if diff_full %}

{{ diff_full | code }}
{%- endif %}
{%- if diff_url %}

{{ diff_url | link }}
{%- endif %}"#;

/// The built-in template for messages in `markup`.
pub fn builtin(markup: Markup) -> &'static str {
    match markup {
        Markup::Plain => PLAIN,
        Markup::TelegramHtml => TELEGRAM,
        Markup::DiscordMarkdown => DISCORD,
        Markup::SlackMrkdwn => SLACK,
    }
}
