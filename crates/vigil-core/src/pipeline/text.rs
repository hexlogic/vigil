//! Steps that work on lines of text.

use std::collections::HashSet;

use regex::Regex;

use super::error::PipelineErrorKind;
use crate::pattern::{Pattern, raw_regex};
use crate::process::MAX_OUTPUT_BYTES;

/// A compiled text step.
#[derive(Debug)]
pub(super) enum TextStep {
    Extract {
        regex: Regex,
        group: usize,
        required: bool,
    },
    Replace {
        regex: Regex,
        replacement: String,
    },
    Ignore(Vec<Pattern>),
    Sort,
    Unique,
    Trim,
    CollapseWhitespace,
    Lowercase,
}

pub(super) fn extract(
    pattern: &str,
    group: usize,
    required: bool,
) -> Result<TextStep, PipelineErrorKind> {
    let regex = raw_regex(pattern)?;
    let groups = regex.captures_len() - 1;
    if group > groups {
        return Err(PipelineErrorKind::NoSuchGroup { group, groups });
    }
    Ok(TextStep::Extract {
        regex,
        group,
        required,
    })
}

pub(super) fn replace(pattern: &str, replacement: &str) -> Result<TextStep, PipelineErrorKind> {
    let regex = raw_regex(pattern)?;
    check_replacement(&regex, replacement)?;
    Ok(TextStep::Replace {
        regex,
        replacement: replacement.to_owned(),
    })
}

/// Refuse a replacement that names a group the pattern does not have.
///
/// The `regex` crate reads `$1USD` as the group called `1USD`, which does not exist, and
/// replaces it with nothing, so the text after the group silently disappears. `${1}USD` is
/// what was meant.
fn check_replacement(regex: &Regex, replacement: &str) -> Result<(), PipelineErrorKind> {
    let mut rest = replacement;
    while let Some(dollar) = rest.find('$') {
        rest = &rest[dollar + 1..];
        if let Some(after) = rest.strip_prefix('$') {
            rest = after;
            continue;
        }
        let (name, after) = match rest.strip_prefix('{') {
            Some(inner) => match inner.find('}') {
                Some(end) => (&inner[..end], &inner[end + 1..]),
                None => continue,
            },
            None => {
                let end = rest
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len());
                (&rest[..end], &rest[end..])
            }
        };
        rest = after;
        if !name.is_empty() && !has_group(regex, name) {
            return Err(PipelineErrorKind::ReplacementGroup {
                name: name.to_owned(),
                hint: group_then_text_hint(regex, name),
            });
        }
    }
    Ok(())
}

fn has_group(regex: &Regex, name: &str) -> bool {
    if name.bytes().all(|b| b.is_ascii_digit()) {
        name.parse::<usize>()
            .is_ok_and(|index| index < regex.captures_len())
    } else {
        regex.capture_names().flatten().any(|group| group == name)
    }
}

/// For `1USD`, where group 1 exists: how to write the group followed by text.
fn group_then_text_hint(regex: &Regex, name: &str) -> String {
    let digits = name.len() - name.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let (number, text) = name.split_at(digits);
    if !number.is_empty() && !text.is_empty() && has_group(regex, number) {
        format!("; to put text right after group {number}, write ${{{number}}}{text}")
    } else {
        String::new()
    }
}

pub(super) fn ignore(patterns: &[String]) -> Result<TextStep, PipelineErrorKind> {
    let compiled: Result<Vec<_>, _> = patterns.iter().map(|p| Pattern::parse(p)).collect();
    Ok(TextStep::Ignore(compiled?))
}

impl TextStep {
    /// Run the step over `text`.
    pub(super) fn apply(&self, text: &str) -> Result<String, PipelineErrorKind> {
        let out = match self {
            Self::Extract {
                regex,
                group,
                required,
            } => extract_matches(regex, *group, *required, text)?,
            Self::Replace { regex, replacement } => replace_matches(regex, replacement, text)?,
            Self::Ignore(patterns) => lines_where(text, |line| {
                !patterns.iter().any(|pattern| pattern.is_match(line))
            }),
            Self::Sort => {
                let mut lines: Vec<&str> = text.lines().collect();
                lines.sort_unstable();
                join(&lines)
            }
            Self::Unique => {
                let mut seen = HashSet::new();
                lines_where(text, |line| seen.insert(line.to_owned()))
            }
            Self::Trim => map_lines(text, |line| line.trim().to_owned()),
            Self::CollapseWhitespace => map_lines(text, collapse_whitespace),
            Self::Lowercase => text.to_lowercase(),
        };
        if out.len() > MAX_OUTPUT_BYTES {
            return Err(PipelineErrorKind::TooLarge);
        }
        Ok(out)
    }
}

fn extract_matches(
    regex: &Regex,
    group: usize,
    required: bool,
    text: &str,
) -> Result<String, PipelineErrorKind> {
    let mut out = String::new();
    for captures in regex.captures_iter(text) {
        let Some(found) = captures.get(group).filter(|found| !found.is_empty()) else {
            continue;
        };
        out.push_str(found.as_str());
        out.push('\n');
        if out.len() > MAX_OUTPUT_BYTES {
            return Err(PipelineErrorKind::TooLarge);
        }
    }
    if out.is_empty() && required {
        return Err(PipelineErrorKind::NoMatch);
    }
    Ok(out)
}

/// Like `Regex::replace_all`, but stops as soon as the output passes the limit.
fn replace_matches(
    regex: &Regex,
    replacement: &str,
    text: &str,
) -> Result<String, PipelineErrorKind> {
    let mut out = String::with_capacity(text.len());
    let mut copied_to = 0;
    for captures in regex.captures_iter(text) {
        let Some(whole) = captures.get(0) else {
            continue;
        };
        out.push_str(&text[copied_to..whole.start()]);
        captures.expand(replacement, &mut out);
        copied_to = whole.end();
        if out.len() > MAX_OUTPUT_BYTES {
            return Err(PipelineErrorKind::TooLarge);
        }
    }
    out.push_str(&text[copied_to..]);
    Ok(out)
}

fn lines_where(text: &str, mut keep: impl FnMut(&str) -> bool) -> String {
    let kept: Vec<&str> = text.lines().filter(|line| keep(line)).collect();
    join(&kept)
}

fn map_lines(text: &str, mut change: impl FnMut(&str) -> String) -> String {
    let changed: Vec<String> = text.lines().map(&mut change).collect();
    join(&changed)
}

fn join<S: AsRef<str>>(lines: &[S]) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(line.as_ref());
        out.push('\n');
    }
    out
}

fn collapse_whitespace(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_run = false;
    for ch in line.chars() {
        if ch.is_whitespace() {
            if !in_run {
                out.push(' ');
            }
            in_run = true;
        } else {
            out.push(ch);
            in_run = false;
        }
    }
    out
}
