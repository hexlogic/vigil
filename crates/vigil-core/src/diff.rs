//! Line diffs between two snapshots.

use std::fmt::Write as _;

use similar::{ChangeTag, TextDiff};

/// How a line differs between the old and the new text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// Only in the new text.
    Added,
    /// Only in the old text.
    Removed,
    /// In both texts.
    Unchanged,
}

/// One line of a diff, without its line terminator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// How the line differs.
    pub kind: ChangeKind,
    /// The line text.
    pub text: String,
}

/// A line-by-line diff of two texts, in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineDiff {
    lines: Vec<DiffLine>,
}

impl LineDiff {
    /// Diff `old` against `new`.
    pub fn between(old: &str, new: &str) -> Self {
        let diff = TextDiff::from_lines(old, new);
        let lines = diff
            .iter_all_changes()
            .map(|change| DiffLine {
                kind: match change.tag() {
                    ChangeTag::Equal => ChangeKind::Unchanged,
                    ChangeTag::Delete => ChangeKind::Removed,
                    ChangeTag::Insert => ChangeKind::Added,
                },
                text: change.value().trim_end_matches(['\n', '\r']).to_owned(),
            })
            .collect();
        Self { lines }
    }

    /// Every line, including unchanged ones.
    pub fn lines(&self) -> &[DiffLine] {
        &self.lines
    }

    /// Lines only present in the new text.
    pub fn added(&self) -> impl Iterator<Item = &str> {
        self.of_kind(ChangeKind::Added)
    }

    /// Lines only present in the old text.
    pub fn removed(&self) -> impl Iterator<Item = &str> {
        self.of_kind(ChangeKind::Removed)
    }

    /// Number of added lines.
    pub fn added_count(&self) -> usize {
        self.added().count()
    }

    /// Number of removed lines.
    pub fn removed_count(&self) -> usize {
        self.removed().count()
    }

    /// True when the two texts have the same lines.
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.kind == ChangeKind::Unchanged)
    }

    fn of_kind(&self, kind: ChangeKind) -> impl Iterator<Item = &str> {
        self.lines
            .iter()
            .filter(move |l| l.kind == kind)
            .map(|l| l.text.as_str())
    }

    /// Render as unified-diff hunks (`@@ -a,b +c,d @@` plus ` `/`+`/`-` lines) with `context`
    /// unchanged lines around each change. Returns an empty string when nothing changed.
    pub fn unified(&self, context: usize) -> String {
        let changed: Vec<usize> = self
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.kind != ChangeKind::Unchanged)
            .map(|(i, _)| i)
            .collect();

        // Merge the context windows around changes into hunk ranges.
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for i in changed {
            let start = i.saturating_sub(context);
            let end = (i + context + 1).min(self.lines.len());
            match ranges.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => ranges.push((start, end)),
            }
        }

        let mut out = String::new();
        for (start, end) in ranges {
            let (old_before, new_before) = self.counts(0, start);
            let (old_len, new_len) = self.counts(start, end);
            let old_start = if old_len == 0 {
                old_before
            } else {
                old_before + 1
            };
            let new_start = if new_len == 0 {
                new_before
            } else {
                new_before + 1
            };
            // Writing to a String cannot fail.
            let _ = writeln!(out, "@@ -{old_start},{old_len} +{new_start},{new_len} @@");
            for line in &self.lines[start..end] {
                let prefix = match line.kind {
                    ChangeKind::Added => '+',
                    ChangeKind::Removed => '-',
                    ChangeKind::Unchanged => ' ',
                };
                let _ = writeln!(out, "{prefix}{}", line.text);
            }
        }
        out
    }

    /// Number of (old, new) lines in `self.lines[from..to]`.
    fn counts(&self, from: usize, to: usize) -> (usize, usize) {
        self.lines[from..to]
            .iter()
            .fold((0, 0), |(old, new), l| match l.kind {
                ChangeKind::Added => (old, new + 1),
                ChangeKind::Removed => (old + 1, new),
                ChangeKind::Unchanged => (old + 1, new + 1),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_texts_have_an_empty_diff() {
        let diff = LineDiff::between("a\nb\n", "a\nb\n");
        assert!(diff.is_empty());
        assert_eq!(diff.unified(3), "");
    }

    #[test]
    fn classifies_added_and_removed_lines() {
        let diff = LineDiff::between("a\nb\nc\n", "a\nB\nc\nd\n");
        assert_eq!(diff.removed().collect::<Vec<_>>(), ["b"]);
        assert_eq!(diff.added().collect::<Vec<_>>(), ["B", "d"]);
        assert_eq!((diff.removed_count(), diff.added_count()), (1, 2));
        assert!(!diff.is_empty());
    }

    #[test]
    fn diff_against_empty_text_is_all_additions() {
        let diff = LineDiff::between("", "x\n");
        assert_eq!(diff.added().collect::<Vec<_>>(), ["x"]);
        assert_eq!(diff.removed_count(), 0);
    }

    #[test]
    fn unified_output_with_context() {
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n";
        let new = "1\n2\n3\nfour\n5\n6\n7\n8\n";
        let expected = "@@ -3,3 +3,3 @@\n 3\n-4\n+four\n 5\n";
        assert_eq!(LineDiff::between(old, new).unified(1), expected);
    }

    #[test]
    fn nearby_changes_share_one_hunk_and_distant_ones_split() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
        let new = "A\nb\nc\nd\ne\nf\ng\nh\ni\nJ\n";
        let two = LineDiff::between(old, new).unified(1);
        assert_eq!(two.matches("@@ ").count(), 2, "{two}");
        let one = LineDiff::between(old, new).unified(10);
        assert_eq!(one.matches("@@ ").count(), 1, "{one}");
        assert!(one.starts_with("@@ -1,10 +1,10 @@\n"), "{one}");
    }

    #[test]
    fn pure_addition_without_context_points_at_the_preceding_line() {
        let diff = LineDiff::between("a\nb\n", "a\nx\nb\n");
        assert_eq!(diff.unified(0), "@@ -1,0 +2,1 @@\n+x\n");
    }

    #[test]
    fn pure_removal_without_context() {
        let diff = LineDiff::between("a\nx\nb\n", "a\nb\n");
        assert_eq!(diff.unified(0), "@@ -2,1 +1,0 @@\n-x\n");
    }
}
