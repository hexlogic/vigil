//! A notification message of the source, written for Vigil's templates.
//!
//! Both use Jinja syntax, so most of a message carries over; the variables have other names.
//! Names are only changed inside `{{ }}` and `{% %}` and outside quoted strings, so prose that
//! happens to hold a word like `diff` is left alone. The result is checked by Vigil's own
//! template validator, and a message that still does not work is not imported.

use vigil_notify::validate_template;

use crate::report::Note;

/// A variable of the source and what stands in its place.
const RENAMES: &[(&str, &str)] = &[
    ("watch_title", "watch_name"),
    ("watch_uuid", "watch_id"),
    ("diff", "diff_full"),
    ("diff_clean", "diff_full"),
    ("diff_full", "diff_full"),
    ("diff_patch", "diff_full"),
    ("diff_added", "diff_added"),
    ("diff_removed", "diff_removed"),
    ("current_snapshot", "current_snapshot"),
    ("base_url", "vigil_base_url"),
    ("diff_url", "diff_url"),
    ("preview_url", "diff_url"),
    ("triggered_text", "triggered_text|join(\"\\n\")"),
];

/// Calls that take options in the source and are plain variables here.
const CALLS: &[(&str, &str)] = &[
    ("diff_added_clean", "diff_added"),
    ("diff_removed_clean", "diff_removed"),
    ("diff_added", "diff_added"),
    ("diff_removed", "diff_removed"),
    ("diff_full", "diff_full"),
    ("diff", "diff_full"),
];

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Rewrite the code between delimiters: names outside strings, and calls with options.
fn rewrite_code(code: &str) -> String {
    let chars: Vec<char> = code.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' || c == '"' {
            // A string: copy it as it is, to its closing quote.
            out.push(c);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                if chars[i] == '\\' && i + 1 < chars.len() {
                    out.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                i += 1;
                if chars[i - 1] == c {
                    break;
                }
            }
            continue;
        }
        if is_word(c) && !c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && is_word(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let after_dot = start > 0 && chars[start - 1] == '.';
            // `name(options)` is a call with options in the source.
            let mut j = i;
            while j < chars.len() && chars[j] == ' ' {
                j += 1;
            }
            if !after_dot
                && j < chars.len()
                && chars[j] == '('
                && let Some((_, to)) = CALLS.iter().find(|(from, _)| *from == word)
                && let Some(close) = chars[j..].iter().position(|&c| c == ')')
            {
                out.push_str(to);
                i = j + close + 1;
                continue;
            }
            match RENAMES.iter().find(|(from, _)| *from == word) {
                Some((_, to)) if !after_dot => out.push_str(to),
                _ => out.push_str(&word),
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Rewrite every `{{ }}` and `{% %}` of `text`; the text between them is not touched.
fn rewrite(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let close = match after.chars().next() {
            Some('{') => "}}",
            Some('%') => "%}",
            Some('#') => "#}",
            _ => {
                out.push_str(&rest[..=open]);
                rest = after;
                continue;
            }
        };
        let Some(end) = after.find(close) else {
            break;
        };
        out.push_str(&rest[..open + 2]);
        let code = &after[1..end];
        if close == "#}" {
            out.push_str(code);
        } else {
            out.push_str(&rewrite_code(code));
        }
        out.push_str(close);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// A Vigil template for the message of a watch, or `None` with a note saying why not.
pub(super) fn translate(
    title: Option<&str>,
    body: Option<&str>,
    notes: &mut Vec<Note>,
) -> Option<String> {
    let title = title.map(str::trim).filter(|t| !t.is_empty());
    let body = body.map(str::trim).filter(|b| !b.is_empty());
    let Some(body) = body else {
        if title.is_some() {
            notes.push(Note::info(
                "notification_title",
                "a custom message title is only imported together with a custom message",
            ));
        }
        return None;
    };
    let mut template = String::new();
    if let Some(title) = title {
        template.push_str("{% set title %}");
        template.push_str(&rewrite(title));
        template.push_str("{% endset %}");
    }
    template.push_str(&rewrite(body));
    match validate_template(&template) {
        Ok(()) => {
            notes.push(Note::info(
                "notification_body",
                "the custom message was rewritten for Vigil's templates; check it with a test notification",
            ));
            Some(template)
        }
        Err(error) => {
            let first_line = error
                .to_string()
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            notes.push(Note::skipped(
                "notification_body",
                format!(
                    "the custom message uses something Vigil's templates do not have and was left out ({})",
                    first_line.chars().take(120).collect::<String>()
                ),
            ));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{rewrite, translate};

    #[test]
    fn variables_are_renamed_inside_tags_and_not_in_prose_or_strings() {
        assert_eq!(
            rewrite(
                "The diff of {{watch_title}} is {{ diff }}. {% if triggered_text %}see {{ 'diff' }}{% endif %}"
            ),
            "The diff of {{watch_name}} is {{ diff_full }}. {% if triggered_text|join(\"\\n\") %}see {{ 'diff' }}{% endif %}"
        );
        assert_eq!(rewrite("{# diff #}x"), "{# diff #}x");
        assert_eq!(rewrite("no tags { here }"), "no tags { here }");
    }

    #[test]
    fn calls_with_options_become_plain_variables() {
        assert_eq!(
            rewrite(
                "{% set d = diff_added_clean(word_diff=false)|trim %}{{ d|replace('\\n', '\\n\\n') }}"
            ),
            "{% set d = diff_added|trim %}{{ d|replace('\\n', '\\n\\n') }}"
        );
        assert_eq!(rewrite("{{ x.diff }}"), "{{ x.diff }}");
    }

    #[test]
    fn a_message_that_works_is_kept_with_its_title_and_one_that_does_not_is_not() {
        let mut notes = Vec::new();
        let template = translate(
            Some("🆕 {{watch_title}}"),
            Some(
                "{% set d = diff_added_clean(word_diff=false)|trim %}{% if d %}{{ d }}{% else %}nothing{% endif %}",
            ),
            &mut notes,
        );
        assert_eq!(
            template.as_deref(),
            Some(
                "{% set title %}🆕 {{watch_name}}{% endset %}{% set d = diff_added|trim %}{% if d %}{{ d }}{% else %}nothing{% endif %}"
            )
        );
        assert_eq!(notes.len(), 1);

        let mut notes = Vec::new();
        assert_eq!(
            translate(None, Some("{{ nope_variable }}"), &mut notes),
            None
        );
        assert!(notes[0].message.contains("left out"), "{notes:?}");
        let mut notes = Vec::new();
        assert_eq!(translate(Some("only a title"), None, &mut notes), None);
        assert_eq!(translate(None, None, &mut Vec::new()), None);
    }
}
