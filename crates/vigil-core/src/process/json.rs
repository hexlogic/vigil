//! Canonical printing of JSON values.

use serde_json::Value;

use super::{MAX_OUTPUT_BYTES, ProcessError};

/// Deepest nesting that is printed. The parser stops at 128 levels; values built by later
/// steps are held to the same limit so printing and dropping them cannot exhaust the stack.
pub(super) const MAX_DEPTH: usize = 128;

const INDENT: &str = "  ";

/// Print `value` with two-space indentation, object keys sorted at every level (by code
/// point), arrays in their original order, and a single trailing newline.
///
/// Keys are sorted here rather than trusted to the map type: `serde_json` keeps insertion
/// order when any crate in the build enables its `preserve_order` feature.
pub(crate) fn render_canonical(value: &Value) -> Result<String, ProcessError> {
    let mut out = Writer(String::new());
    write_value(&mut out, value, 0)?;
    out.push("\n")?;
    Ok(out.0)
}

/// A string that refuses to grow past [`MAX_OUTPUT_BYTES`].
struct Writer(String);

impl Writer {
    fn push(&mut self, text: &str) -> Result<(), ProcessError> {
        if self.0.len() + text.len() > MAX_OUTPUT_BYTES {
            return Err(ProcessError::TooLarge);
        }
        self.0.push_str(text);
        Ok(())
    }

    fn indent(&mut self, depth: usize) -> Result<(), ProcessError> {
        for _ in 0..depth {
            self.push(INDENT)?;
        }
        Ok(())
    }

    fn string(&mut self, text: &str) -> Result<(), ProcessError> {
        self.push(&serde_json::to_string(text)?)
    }
}

fn write_value(out: &mut Writer, value: &Value, depth: usize) -> Result<(), ProcessError> {
    if depth > MAX_DEPTH {
        return Err(ProcessError::TooDeep);
    }
    match value {
        Value::Null => out.push("null"),
        Value::Bool(true) => out.push("true"),
        Value::Bool(false) => out.push("false"),
        Value::Number(number) => out.push(&number.to_string()),
        Value::String(text) => out.string(text),
        Value::Array(items) => {
            if items.is_empty() {
                return out.push("[]");
            }
            out.push("[\n")?;
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(",\n")?;
                }
                out.indent(depth + 1)?;
                write_value(out, item, depth + 1)?;
            }
            out.push("\n")?;
            out.indent(depth)?;
            out.push("]")
        }
        Value::Object(map) => {
            if map.is_empty() {
                return out.push("{}");
            }
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push("{\n")?;
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(",\n")?;
                }
                out.indent(depth + 1)?;
                out.string(key)?;
                out.push(": ")?;
                write_value(out, item, depth + 1)?;
            }
            out.push("\n")?;
            out.indent(depth)?;
            out.push("}")
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::{MAX_DEPTH, render_canonical};
    use crate::process::ProcessError;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn keys_are_sorted_at_every_level() -> TestResult {
        let mut inner = Map::new();
        for key in ["z", "m", "a"] {
            inner.insert(key.to_owned(), json!(1));
        }
        let mut outer = Map::new();
        for key in ["b", "c", "a"] {
            outer.insert(key.to_owned(), Value::Object(inner.clone()));
        }
        let text = render_canonical(&Value::Object(outer))?;
        let keys: Vec<&str> = text
            .lines()
            .filter_map(|line| line.trim().split(':').next())
            .filter(|key| key.starts_with('"'))
            .collect();
        assert_eq!(
            keys,
            [
                "\"a\"", "\"a\"", "\"m\"", "\"z\"", "\"b\"", "\"a\"", "\"m\"", "\"z\"", "\"c\"",
                "\"a\"", "\"m\"", "\"z\""
            ]
        );
        Ok(())
    }

    #[test]
    fn arrays_keep_their_order() -> TestResult {
        assert_eq!(
            render_canonical(&json!([3, 1, 2]))?,
            "[\n  3,\n  1,\n  2\n]\n"
        );
        Ok(())
    }

    #[test]
    fn empty_containers_stay_on_one_line() -> TestResult {
        assert_eq!(
            render_canonical(&json!({"a": [], "b": {}}))?,
            "{\n  \"a\": [],\n  \"b\": {}\n}\n"
        );
        Ok(())
    }

    #[test]
    fn scalars_are_printed_on_their_own() -> TestResult {
        assert_eq!(render_canonical(&json!(null))?, "null\n");
        assert_eq!(render_canonical(&json!(true))?, "true\n");
        assert_eq!(render_canonical(&json!("x"))?, "\"x\"\n");
        assert_eq!(render_canonical(&json!(-12))?, "-12\n");
        assert_eq!(render_canonical(&json!(1.5))?, "1.5\n");
        Ok(())
    }

    #[test]
    fn strings_are_escaped_but_unicode_is_kept() -> TestResult {
        let text = render_canonical(&json!({"q": "a\"b\\c\nd\u{1}", "ua": "Київ"}))?;
        assert!(text.contains(r#""q": "a\"b\\c\nd\u0001""#), "{text}");
        assert!(text.contains(r#""ua": "Київ""#), "{text}");
        Ok(())
    }

    #[test]
    fn printing_is_idempotent() -> TestResult {
        let once = render_canonical(&json!({"b": [1, {"d": 1, "c": 2}], "a": "x"}))?;
        let again = render_canonical(&serde_json::from_str(&once)?)?;
        assert_eq!(once, again);
        Ok(())
    }

    #[test]
    fn nesting_beyond_the_limit_is_an_error() -> TestResult {
        let mut value = json!(1);
        for _ in 0..=MAX_DEPTH {
            value = Value::Array(vec![value]);
        }
        assert!(matches!(
            render_canonical(&value),
            Err(ProcessError::TooDeep)
        ));
        let mut value = json!(1);
        for _ in 0..MAX_DEPTH {
            value = Value::Array(vec![value]);
        }
        render_canonical(&value)?;
        Ok(())
    }

    #[test]
    fn output_beyond_the_limit_is_an_error() {
        let big = Value::String("x".repeat(super::MAX_OUTPUT_BYTES));
        assert!(matches!(
            render_canonical(&big),
            Err(ProcessError::TooLarge)
        ));
    }
}
