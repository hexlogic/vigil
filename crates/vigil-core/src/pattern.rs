//! Text patterns shared by pipeline steps and conditions.
//!
//! A pattern is either a plain substring, matched case-insensitively, or `/regex/flags`. The
//! regex form needs a non-empty body between two slashes and only the flags `i`, `m`, `s`, `x`
//! and `U` after the last slash. Anything else is a plain substring, so `//`, `/` and `/a/g`
//! are all plain text, while `/api/v1/` is the regex `api/v1`.

use regex::{Regex, RegexBuilder};

/// Compiled size limit for every regex Vigil builds, so a hostile or careless pattern cannot
/// use unbounded memory.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// Flags accepted after the closing slash of a `/regex/flags` pattern.
const FLAGS: &str = "imsxU";

/// A pattern could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
    /// The pattern is empty or only whitespace, which would match everything.
    #[error("the pattern is empty")]
    Empty,
    /// The regex is invalid or too large.
    #[error("invalid regex {pattern:?}: {reason}")]
    Regex {
        /// The text as given.
        pattern: String,
        /// One-line description of the problem.
        reason: String,
    },
}

/// A compiled pattern.
#[derive(Debug, Clone)]
pub struct Pattern {
    source: String,
    regex: Regex,
}

impl Pattern {
    /// Compile `source`, which is a plain substring or `/regex/flags`.
    pub fn parse(source: &str) -> Result<Self, PatternError> {
        if source.trim().is_empty() {
            return Err(PatternError::Empty);
        }
        let built = match split_regex(source) {
            Some((body, flags)) => build_regex(body, flags),
            None => build_regex(&regex::escape(source), "i"),
        };
        let regex = built.map_err(|error| PatternError::Regex {
            pattern: source.to_owned(),
            reason: reason(&error),
        })?;
        Ok(Self {
            source: source.to_owned(),
            regex,
        })
    }

    /// Whether `text` contains a match.
    pub fn is_match(&self, text: &str) -> bool {
        self.regex.is_match(text)
    }

    /// The pattern as it was written.
    pub fn as_str(&self) -> &str {
        &self.source
    }
}

/// Split `/body/flags` into its body and flags, or `None` for a plain pattern.
fn split_regex(source: &str) -> Option<(&str, &str)> {
    let inner = source.strip_prefix('/')?;
    let close = inner.rfind('/')?;
    let (body, flags) = (&inner[..close], &inner[close + 1..]);
    (!body.is_empty() && flags.chars().all(|flag| FLAGS.contains(flag))).then_some((body, flags))
}

/// Build a size-limited regex from raw regex syntax and a string of inline flags.
///
/// Repeated flags are accepted and applied once.
pub(crate) fn build_regex(body: &str, flags: &str) -> Result<Regex, regex::Error> {
    let mut unique = String::new();
    for flag in flags.chars().filter(|flag| FLAGS.contains(*flag)) {
        if !unique.contains(flag) {
            unique.push(flag);
        }
    }
    let source = if unique.is_empty() {
        body.to_owned()
    } else {
        format!("(?{unique}){body}")
    };
    RegexBuilder::new(&source)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_SIZE_LIMIT)
        .build()
}

/// Compile raw regex syntax (no `/.../` wrapper, no substring handling) with the shared limits.
pub(crate) fn raw_regex(pattern: &str) -> Result<Regex, PatternError> {
    build_regex(pattern, "").map_err(|error| PatternError::Regex {
        pattern: pattern.to_owned(),
        reason: reason(&error),
    })
}

/// The last line of a regex error, which is the explanation without the multi-line diagram.
pub(crate) fn reason(error: &regex::Error) -> String {
    let text = error.to_string();
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("invalid regex");
    line.strip_prefix("error: ").unwrap_or(line).to_owned()
}

#[cfg(test)]
mod tests {
    use super::{Pattern, PatternError, build_regex};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn matches(pattern: &str, text: &str) -> Result<bool, PatternError> {
        Ok(Pattern::parse(pattern)?.is_match(text))
    }

    #[test]
    fn plain_is_a_case_insensitive_substring() -> TestResult {
        assert!(matches("sold out", "Item is SOLD OUT today")?);
        assert!(!matches("sold out", "in stock")?);
        Ok(())
    }

    #[test]
    fn plain_text_is_not_a_regex() -> TestResult {
        assert!(matches("1.5 (approx)", "price 1.5 (approx) only")?);
        assert!(!matches("a.c", "abc")?);
        assert!(matches("a+b", "xa+bx")?);
        Ok(())
    }

    #[test]
    fn plain_folds_cyrillic_case() -> TestResult {
        assert!(matches("ТЕНДЕР", "Відкритий тендер на закупівлю")?);
        assert!(matches("тендер", "ТЕНДЕР")?);
        Ok(())
    }

    #[test]
    fn regex_form_is_case_sensitive_without_the_flag() -> TestResult {
        assert!(matches("/price: \\d+/", "price: 42")?);
        assert!(!matches("/price: \\d+/", "PRICE: 42")?);
        assert!(matches("/price: \\d+/i", "PRICE: 42")?);
        Ok(())
    }

    #[test]
    fn regex_flags_apply() -> TestResult {
        assert!(matches("/^b$/m", "a\nb\nc")?);
        assert!(!matches("/^b$/", "a\nb\nc")?);
        assert!(matches("/a.b/s", "a\nb")?);
        assert!(matches("/a b/x", "ab")?);
        assert!(matches("/a+?/U", "aaa")?);
        Ok(())
    }

    #[test]
    fn repeated_flags_are_applied_once() -> TestResult {
        assert!(matches("/abc/ii", "ABC")?);
        Ok(())
    }

    #[test]
    fn unknown_flags_make_the_pattern_plain() -> TestResult {
        assert!(matches("/foo/g", "see /foo/g here")?);
        assert!(!matches("/foo/g", "foo")?);
        Ok(())
    }

    #[test]
    fn slashes_without_a_body_are_plain() -> TestResult {
        assert!(matches("//", "a // b")?);
        assert!(matches("/", "a/b")?);
        assert!(matches("/abc", "x /abc y")?);
        Ok(())
    }

    #[test]
    fn a_path_between_slashes_is_a_regex() -> TestResult {
        // Documented ambiguity: `/api/v1/` reads as the regex `api/v1`.
        let pattern = Pattern::parse("/api/v1/")?;
        assert!(pattern.is_match("GET api/v1"));
        assert!(pattern.is_match("GET /api/v1/items"));
        assert!(!pattern.is_match("GET /API/V1/items"));
        Ok(())
    }

    #[test]
    fn empty_patterns_are_rejected() {
        assert_eq!(Pattern::parse("").err(), Some(PatternError::Empty));
        assert_eq!(Pattern::parse("  \t").err(), Some(PatternError::Empty));
    }

    #[test]
    fn invalid_regex_is_a_one_line_error() {
        let message = Pattern::parse("/(unclosed/")
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(
            message.starts_with("invalid regex \"/(unclosed/\": "),
            "{message}"
        );
        assert!(!message.contains('\n'), "{message}");
    }

    #[test]
    fn oversized_regex_is_rejected() {
        let huge = "/(\\w{100}){100}/";
        assert!(matches!(
            Pattern::parse(huge),
            Err(PatternError::Regex { .. })
        ));
        assert!(build_regex("a{1000}{1000}", "").is_err());
    }

    #[test]
    fn the_source_is_kept() -> TestResult {
        assert_eq!(Pattern::parse("/a|b/i")?.as_str(), "/a|b/i");
        Ok(())
    }
}
