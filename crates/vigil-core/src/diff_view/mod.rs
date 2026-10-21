//! A diff shaped for showing to a person: hunks of numbered lines, with the words that changed
//! inside a changed line picked out.
//!
//! [`LineDiff`](crate::LineDiff) answers "what was added or removed"; this answers "what does
//! the page for it look like". Both the inline (unified) and the side-by-side layout are built
//! from the same [`DiffView`].

use std::time::{Duration, Instant};

use similar::{ChangeTag, DiffOp, DiffTag, TextDiff};

/// Most lines a view holds. A diff of two unrelated pages would otherwise be as long as both.
const MAX_SHOWN_LINES: usize = 5_000;

/// Longest line shown, in characters; the rest is cut off.
const MAX_LINE_CHARS: usize = 2_000;

/// Largest changed block, in bytes, whose words are compared. Bigger blocks are shown as
/// whole lines: refining them costs time and the result would not be readable anyway.
const INLINE_MAX_BYTES: usize = 32 * 1024;

/// Time all word-level comparisons of one view may take together.
const INLINE_BUDGET: Duration = Duration::from_millis(300);

/// A piece of a line, either unchanged or one of the words that differ from the other side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// The text.
    pub text: String,
    /// True for text that changed within a line that has a counterpart on the other side.
    pub emphasized: bool,
}

/// One line of a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Line number in the old text, counted from 1; `None` for an added line.
    pub old_number: Option<usize>,
    /// Line number in the new text, counted from 1; `None` for a removed line.
    pub new_number: Option<usize>,
    /// The line, without its terminator, in pieces.
    pub spans: Vec<Span>,
}

impl Line {
    /// The whole line as one string.
    pub fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }
}

/// A run of lines that is the same on both sides, or a run that differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Lines present in both texts.
    Same(Vec<Line>),
    /// Lines that were replaced: `removed` is the old side, `added` the new side. Either may
    /// be empty.
    Changed {
        /// The lines only the old text has.
        removed: Vec<Line>,
        /// The lines only the new text has.
        added: Vec<Line>,
    },
}

impl Block {
    /// The block as rows of a side-by-side view: old line on the left, new line on the right.
    ///
    /// Unchanged lines are on both sides. In a changed block the i-th removed line is paired
    /// with the i-th added line; whichever side is longer ends in rows with the other side
    /// empty.
    pub fn rows(&self) -> Vec<(Option<&Line>, Option<&Line>)> {
        match self {
            Block::Same(lines) => lines.iter().map(|line| (Some(line), Some(line))).collect(),
            Block::Changed { removed, added } => (0..removed.len().max(added.len()))
                .map(|i| (removed.get(i), added.get(i)))
                .collect(),
        }
    }
}

/// A stretch of the diff: changes with their surrounding unchanged lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// Unchanged lines left out between the previous hunk (or the start of the text) and
    /// this one.
    pub skipped_before: usize,
    /// The hunk's lines, in order.
    pub blocks: Vec<Block>,
}

/// A diff ready to be laid out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffView {
    /// The changes with their context; empty when the texts have the same lines.
    pub hunks: Vec<Hunk>,
    /// Lines in the new text that the old text does not have, in all of the diff.
    pub added: usize,
    /// Lines in the old text that the new text does not have, in all of the diff.
    pub removed: usize,
    /// Unchanged lines left out after the last hunk. 0 when the view was cut short.
    pub skipped_after: usize,
    /// True when the view leaves out changes or part of a line to stay a readable size.
    pub truncated: bool,
}

impl DiffView {
    /// Diff `old` against `new`, keeping `context` unchanged lines around each change.
    pub fn between(old: &str, new: &str, context: usize) -> Self {
        let diff = TextDiff::from_lines(old, new);
        let (mut added, mut removed) = (0, 0);
        for op in diff.ops() {
            if op.tag() != DiffTag::Equal {
                removed += op.old_range().len();
                added += op.new_range().len();
            }
        }

        let mut build = Builder {
            diff: &diff,
            deadline: Instant::now() + INLINE_BUDGET,
            shown: 0,
            truncated: false,
        };
        let mut hunks = Vec::new();
        let mut covered = 0;
        let mut cut_short = false;
        let groups = diff.grouped_ops(context);
        let group_count = groups.len();
        for (index, group) in groups.into_iter().enumerate() {
            let (Some(first), Some(last)) = (group.first(), group.last()) else {
                continue;
            };
            let hunk = Hunk {
                skipped_before: first.old_range().start.saturating_sub(covered),
                blocks: build.blocks(&group),
            };
            covered = last.old_range().end;
            if !hunk.blocks.is_empty() {
                hunks.push(hunk);
            }
            if build.shown >= MAX_SHOWN_LINES && index + 1 < group_count {
                cut_short = true;
                break;
            }
        }
        // A view that stopped early does not know what follows, so it claims nothing.
        let truncated = build.truncated || cut_short;
        let skipped_after = if truncated {
            0
        } else {
            diff.old_len().saturating_sub(covered)
        };
        Self {
            hunks,
            added,
            removed,
            skipped_after,
            truncated,
        }
    }

    /// True when the two texts have the same lines.
    pub fn is_empty(&self) -> bool {
        self.hunks.is_empty()
    }
}

/// The pieces of a line, each marked as emphasized or not.
type Parts<'s> = Vec<(bool, &'s str)>;

/// A line waiting to be turned into a [`Line`]: its old and new index, counted from 0, and
/// its pieces.
type Pending<'s> = (Option<usize>, Option<usize>, Parts<'s>);

/// State shared while the blocks of one view are built.
struct Builder<'a, 'old, 'new> {
    diff: &'a TextDiff<'old, 'new, str>,
    deadline: Instant,
    shown: usize,
    truncated: bool,
}

impl Builder<'_, '_, '_> {
    /// The blocks for the ops of one hunk, stopping when the view is full.
    fn blocks(&mut self, ops: &[DiffOp]) -> Vec<Block> {
        let mut blocks = Vec::new();
        for op in ops {
            if self.shown >= MAX_SHOWN_LINES {
                self.truncated = true;
                break;
            }
            let block = match op.tag() {
                DiffTag::Equal => Block::Same(self.same(op)),
                DiffTag::Delete | DiffTag::Insert | DiffTag::Replace => self.changed(op),
            };
            // Zero context leaves empty runs of unchanged lines at the edges of a hunk.
            if matches!(&block, Block::Same(lines) if lines.is_empty()) {
                continue;
            }
            blocks.push(block);
        }
        blocks
    }

    fn same(&mut self, op: &DiffOp) -> Vec<Line> {
        let diff = self.diff;
        let mut lines = Vec::new();
        for change in diff.iter_changes(op) {
            if self.shown >= MAX_SHOWN_LINES {
                self.truncated = true;
                break;
            }
            self.shown += 1;
            lines.push(self.line(
                change.old_index(),
                change.new_index(),
                vec![(false, change.value())],
            ));
        }
        lines
    }

    fn changed(&mut self, op: &DiffOp) -> Block {
        let diff = self.diff;
        let refine = op.tag() == DiffTag::Replace && self.small_enough(op);
        let mut removed = Vec::new();
        let mut added = Vec::new();
        if refine {
            for change in diff.iter_inline_changes_deadline(op, Some(self.deadline)) {
                let parts = change.values().to_vec();
                match change.tag() {
                    ChangeTag::Delete => removed.push((change.old_index(), None, parts)),
                    ChangeTag::Insert => added.push((None, change.new_index(), parts)),
                    ChangeTag::Equal => {}
                }
            }
        } else {
            for change in diff.iter_changes(op) {
                let parts = vec![(false, change.value())];
                match change.tag() {
                    ChangeTag::Delete => removed.push((change.old_index(), None, parts)),
                    ChangeTag::Insert => added.push((None, change.new_index(), parts)),
                    ChangeTag::Equal => {}
                }
            }
        }
        Block::Changed {
            removed: self.take(removed),
            added: self.take(added),
        }
    }

    /// Turn the collected lines into [`Line`]s while room is left in the view.
    fn take(&mut self, parts: Vec<Pending<'_>>) -> Vec<Line> {
        let mut lines = Vec::new();
        for (old, new, spans) in parts {
            if self.shown >= MAX_SHOWN_LINES {
                self.truncated = true;
                break;
            }
            self.shown += 1;
            lines.push(self.line(old, new, spans));
        }
        lines
    }

    /// Whether the words of a replaced block are worth comparing.
    fn small_enough(&self, op: &DiffOp) -> bool {
        let old = op.old_range().filter_map(|i| self.diff.old_slice(i));
        let new = op.new_range().filter_map(|i| self.diff.new_slice(i));
        let bytes: usize = old.chain(new).map(str::len).sum();
        bytes <= INLINE_MAX_BYTES && Instant::now() < self.deadline
    }

    /// A line from pieces that still carry the line terminator, which is dropped. Indexes
    /// count from 0 in `similar` and from 1 on screen.
    fn line(&mut self, old: Option<usize>, new: Option<usize>, parts: Parts<'_>) -> Line {
        let mut spans: Vec<Span> = Vec::new();
        let mut chars = 0;
        let mut clipped = false;
        for (emphasized, text) in parts {
            let text = text.trim_end_matches(['\n', '\r']);
            let room = MAX_LINE_CHARS - chars;
            let kept: String = text.chars().take(room).collect();
            chars += kept.chars().count();
            clipped |= kept.len() < text.len();
            match spans.last_mut() {
                Some(last) if last.emphasized == emphasized => last.text.push_str(&kept),
                _ if kept.is_empty() => {}
                _ => spans.push(Span {
                    text: kept,
                    emphasized,
                }),
            }
            if clipped {
                break;
            }
        }
        if clipped {
            self.truncated = true;
            spans.push(Span {
                text: "…".to_owned(),
                emphasized: false,
            });
        }
        // A line that changed all through is marked by its row alone.
        if let [only] = spans.as_mut_slice() {
            only.emphasized = false;
        }
        Line {
            old_number: old.map(|i| i + 1),
            new_number: new.map(|i| i + 1),
            spans,
        }
    }
}

#[cfg(test)]
mod tests;
