//! A [`DiffView`] shaped for the templates: plain structures, text already translated.

use vigil_core::{Block, DiffView, Line};

use crate::i18n::Translator;

/// A piece of a line; `marked` for the words that changed.
pub(crate) struct SpanView {
    pub(crate) text: String,
    pub(crate) marked: bool,
}

/// One line of a diff, with its numbers (empty where the line is not in that text).
pub(crate) struct LineView {
    /// `add`, `del` or empty, the class of the row.
    pub(crate) class: &'static str,
    /// The sign shown in front, and read out as `sign_word`.
    pub(crate) sign: &'static str,
    pub(crate) sign_word: String,
    pub(crate) old: String,
    pub(crate) new: String,
    pub(crate) spans: Vec<SpanView>,
}

/// One entry of the inline layout: a note about lines left out, or a line.
pub(crate) struct InlineItem {
    pub(crate) gap: Option<String>,
    pub(crate) line: Option<LineView>,
}

/// The inline (unified) layout.
pub(crate) struct Inline {
    pub(crate) items: Vec<InlineItem>,
    /// The note after the last hunk about the lines left out.
    pub(crate) tail_gap: Option<String>,
    pub(crate) truncated: bool,
}

fn spans(line: &Line) -> Vec<SpanView> {
    line.spans
        .iter()
        .map(|span| SpanView {
            text: span.text.clone(),
            marked: span.emphasized,
        })
        .collect()
}

fn number(n: Option<usize>) -> String {
    n.map(|n| n.to_string()).unwrap_or_default()
}

fn gap(t: &Translator, lines: usize) -> Option<String> {
    (lines > 0).then(|| t.plural("diff.unchanged_lines", lines))
}

fn line_view(t: &Translator, line: &Line, class: &'static str) -> LineView {
    let (sign, word) = match class {
        "add" => ("+", t.t("diff.added")),
        "del" => ("−", t.t("diff.removed")),
        _ => (" ", String::new()),
    };
    LineView {
        class,
        sign,
        sign_word: word,
        old: number(line.old_number),
        new: number(line.new_number),
        spans: spans(line),
    }
}

/// The inline layout: unchanged lines as they are, then for each change the removed lines
/// followed by the added ones.
pub(crate) fn inline(t: &Translator, view: &DiffView) -> Inline {
    let mut items = Vec::new();
    for hunk in &view.hunks {
        if let Some(note) = gap(t, hunk.skipped_before) {
            items.push(InlineItem {
                gap: Some(note),
                line: None,
            });
        }
        for block in &hunk.blocks {
            let lines: Vec<(&Line, &'static str)> = match block {
                Block::Same(lines) => lines.iter().map(|line| (line, "")).collect(),
                Block::Changed { removed, added } => removed
                    .iter()
                    .map(|line| (line, "del"))
                    .chain(added.iter().map(|line| (line, "add")))
                    .collect(),
            };
            for (line, class) in lines {
                items.push(InlineItem {
                    gap: None,
                    line: Some(line_view(t, line, class)),
                });
            }
        }
    }
    Inline {
        items,
        tail_gap: gap(t, view.skipped_after),
        truncated: view.truncated,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::i18n::{I18n, Lang};

    fn translator() -> Translator {
        Translator::new(Arc::new(I18n::load().expect("locales parse")), Lang::En)
    }

    fn summary(inline: &Inline) -> Vec<String> {
        inline
            .items
            .iter()
            .map(|item| match (&item.gap, &item.line) {
                (Some(gap), _) => format!("~ {gap}"),
                (None, Some(line)) => {
                    let text: String = line
                        .spans
                        .iter()
                        .map(|s| {
                            if s.marked {
                                format!("[{}]", s.text)
                            } else {
                                s.text.clone()
                            }
                        })
                        .collect();
                    format!("{}{}/{} {text}", line.sign, line.old, line.new)
                }
                _ => String::new(),
            })
            .collect()
    }

    #[test]
    fn the_inline_layout_lists_removed_lines_before_added_ones() {
        let view = DiffView::between(
            "a\nPrice 10\nb\nc\nd\ne\nf\n",
            "a\nPrice 12\nb\nc\nd\ne\nf\n",
            1,
        );
        let inline = inline(&translator(), &view);
        assert_eq!(
            summary(&inline),
            [" 1/1 a", "−2/ Price [10]", "+/2 Price [12]", " 3/3 b"]
        );
        assert_eq!(inline.tail_gap.as_deref(), Some("4 unchanged lines"));
        assert!(!inline.truncated);
        assert_eq!(inline.items[1].line.as_ref().map(|l| l.class), Some("del"));
        assert_eq!(
            inline.items[2].line.as_ref().map(|l| l.sign_word.as_str()),
            Some("added")
        );
    }

    #[test]
    fn a_gap_before_a_hunk_is_a_note() {
        let old: String = (1..=10).map(|n| format!("{n}\n")).collect();
        let new = old.replace("9\n", "nine\n");
        let inline = inline(&translator(), &DiffView::between(&old, &new, 1));
        assert_eq!(summary(&inline)[0], "~ 7 unchanged lines");
    }
}
