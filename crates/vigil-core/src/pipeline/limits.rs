//! Limits on the size of selectors, expressions and programs.
//!
//! The parsers and evaluators for CSS selectors, XPath and jq recurse, and a stack overflow
//! aborts the whole process; it cannot be caught. A release build overflowed a 2 MiB stack at
//! about 1,000 chained jq operators or 500 nested `:not(`, so each kind of input is cut off
//! well below what even the 8 MiB main thread, where the command line checks steps, can take.
//! Real selectors, expressions and programs are a few dozen bytes.

/// Longest CSS selector, in bytes.
pub(super) const MAX_SELECTOR_BYTES: usize = 1024;
/// Longest XPath expression, in bytes.
pub(super) const MAX_XPATH_BYTES: usize = 1024;
/// Longest jq program, in bytes.
pub(super) const MAX_JQ_BYTES: usize = 2048;
/// Deepest nesting of `(`, `[` and `{` in any of them.
pub(super) const MAX_NESTING: usize = 32;

/// Why `text` is too big to hand to a parser, or `None` if it is fine. `what` names it
/// ("selector", "expression", "program") and `max_bytes` is its length limit.
pub(super) fn too_big(what: &str, text: &str, max_bytes: usize) -> Option<String> {
    if text.len() > max_bytes {
        return Some(format!("the {what} is longer than {max_bytes} bytes"));
    }
    let (mut depth, mut deepest) = (0usize, 0usize);
    for ch in text.chars() {
        match ch {
            '(' | '[' | '{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    (deepest > MAX_NESTING).then(|| format!("brackets are nested deeper than {MAX_NESTING} levels"))
}

#[cfg(test)]
mod tests {
    use super::{MAX_NESTING, too_big};

    #[test]
    fn short_flat_input_is_fine() {
        assert_eq!(too_big("selector", "div > p.a:not(.b)", 1024), None);
        assert_eq!(too_big("program", "", 10), None);
    }

    #[test]
    fn long_input_is_refused() {
        assert_eq!(
            too_big("program", &"-".repeat(11), 10).as_deref(),
            Some("the program is longer than 10 bytes")
        );
        assert_eq!(too_big("program", &"-".repeat(10), 10), None);
    }

    #[test]
    fn deep_brackets_are_refused_whatever_their_kind() {
        for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
            let text = format!(
                "{}x{}",
                open_n(open, MAX_NESTING + 1),
                open_n(close, MAX_NESTING + 1)
            );
            assert!(too_big("expression", &text, 4096).is_some(), "{open}");
            let ok = format!(
                "{}x{}",
                open_n(open, MAX_NESTING),
                open_n(close, MAX_NESTING)
            );
            assert_eq!(too_big("expression", &ok, 4096), None, "{open}");
        }
    }

    #[test]
    fn many_shallow_brackets_are_fine() {
        assert_eq!(too_big("expression", &"(a)[b]{c}".repeat(100), 4096), None);
    }

    fn open_n(ch: char, n: usize) -> String {
        ch.to_string().repeat(n)
    }
}
