//! Cutting text to a size limit without splitting a character.

/// A channel's size limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// At most this many characters.
    Chars(usize),
    /// At most this many bytes of UTF-8.
    Bytes(usize),
}

impl Limit {
    /// The limit itself.
    pub fn max(self) -> usize {
        match self {
            Self::Chars(n) | Self::Bytes(n) => n,
        }
    }

    /// How much of the limit `text` uses.
    pub fn measure(self, text: &str) -> usize {
        match self {
            Self::Chars(_) => text.chars().count(),
            Self::Bytes(_) => text.len(),
        }
    }

    /// Whether `text` is within the limit.
    pub fn fits(self, text: &str) -> bool {
        self.measure(text) <= self.max()
    }
}

/// The longest prefix of `text` that uses at most `max` units of `limit`.
pub(crate) fn cut_to(text: &str, max: usize, limit: Limit) -> &str {
    let end = match limit {
        Limit::Chars(_) => text.char_indices().nth(max).map_or(text.len(), |(i, _)| i),
        Limit::Bytes(_) => {
            let mut end = max.min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            end
        }
    };
    &text[..end]
}

/// `text` cut to at most `max` units of `limit`, ending in `…` when anything was cut. The cut
/// falls on a line boundary if one is reasonably close.
pub(crate) fn shrink(text: &str, max: usize, limit: Limit) -> String {
    if limit.measure(text) <= max {
        return text.to_owned();
    }
    let ellipsis = "…";
    let room = max.saturating_sub(limit.measure(ellipsis));
    if room == 0 {
        return String::new();
    }
    let mut cut = cut_to(text, room, limit);
    if let Some(newline) = cut.rfind('\n')
        && newline >= cut.len() / 2
    {
        cut = &cut[..newline];
    }
    format!("{cut}{ellipsis}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_fall_on_character_boundaries() {
        // Each Cyrillic letter is 2 bytes, the emoji 4.
        assert_eq!(cut_to("абвгд", 3, Limit::Chars(3)), "абв");
        assert_eq!(cut_to("абвгд", 5, Limit::Bytes(5)), "аб");
        assert_eq!(cut_to("a🙂b", 4, Limit::Bytes(4)), "a");
        assert_eq!(cut_to("a🙂b", 5, Limit::Bytes(5)), "a🙂");
        assert_eq!(cut_to("short", 99, Limit::Bytes(99)), "short");
        assert_eq!(cut_to("абв", 0, Limit::Bytes(0)), "");
    }

    #[test]
    fn shrink_marks_the_cut_and_stays_within_the_limit() {
        for limit in [Limit::Chars(10), Limit::Bytes(10)] {
            let out = shrink("абвгдежзиклмн", 10, limit);
            assert!(out.ends_with('…'), "{out}");
            assert!(limit.fits(&out), "{out}");
        }
        assert_eq!(shrink("fits", 10, Limit::Chars(10)), "fits");
        assert_eq!(shrink("too long", 0, Limit::Chars(0)), "");
    }

    #[test]
    fn shrink_prefers_a_line_boundary() {
        let out = shrink("first line\nsecond line", 18, Limit::Chars(18));
        assert_eq!(out, "first line…");
        // A boundary near the start would throw away too much; cut mid-line instead.
        let out = shrink("a\nsecond line is long", 12, Limit::Chars(12));
        assert_eq!(out, "a\nsecond li…");
    }
}
