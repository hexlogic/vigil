//! Tests for the diff view.

use super::*;

/// One line per diff line: `  1  1 text`, `- 2    old`, `+    2 new`; emphasized text in
/// square brackets.
fn show(view: &DiffView) -> String {
    let mut out = String::new();
    for hunk in &view.hunks {
        out.push_str(&format!("[skip {}]\n", hunk.skipped_before));
        for block in &hunk.blocks {
            let (same, removed, added): (&[Line], &[Line], &[Line]) = match block {
                Block::Same(lines) => (lines, &[], &[]),
                Block::Changed { removed, added } => (&[], removed, added),
            };
            for (mark, lines) in [(' ', same), ('-', removed), ('+', added)] {
                for line in lines {
                    let number = |n: Option<usize>| n.map_or("-".to_owned(), |n| n.to_string());
                    let text: String = line
                        .spans
                        .iter()
                        .map(|s| {
                            if s.emphasized {
                                format!("[{}]", s.text)
                            } else {
                                s.text.clone()
                            }
                        })
                        .collect();
                    out.push_str(&format!(
                        "{mark} {}/{} {text}\n",
                        number(line.old_number),
                        number(line.new_number)
                    ));
                }
            }
        }
    }
    out.push_str(&format!("[skip {}]", view.skipped_after));
    out
}

#[test]
fn identical_texts_have_no_hunks() {
    let view = DiffView::between("a\nb\n", "a\nb\n", 3);
    assert!(view.is_empty());
    assert_eq!((view.added, view.removed, view.truncated), (0, 0, false));
}

#[test]
fn a_changed_line_is_numbered_and_its_words_are_picked_out() {
    let view = DiffView::between(
        "one\nPrice: $10 today\nthree\n",
        "one\nPrice: $12 today\nthree\n",
        1,
    );
    assert_eq!(
        show(&view),
        "[skip 0]\n  1/1 one\n- 2/- Price: [$10] today\n+ -/2 Price: [$12] today\n  3/3 three\n[skip 0]"
    );
    assert_eq!((view.added, view.removed), (1, 1));
}

#[test]
fn context_limits_the_lines_around_a_change_and_the_rest_is_counted() {
    let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    let new = "1\n2\n3\n4\nfive\n6\n7\n8\n9\n10\n";
    let view = DiffView::between(old, new, 2);
    assert_eq!(
        show(&view),
        "[skip 2]\n  3/3 3\n  4/4 4\n- 5/- 5\n+ -/5 five\n  6/6 6\n  7/7 7\n[skip 3]"
    );
}

#[test]
fn distant_changes_make_separate_hunks_that_say_how_much_lies_between() {
    let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
    let new = "A\nb\nc\nd\ne\nf\ng\nh\ni\nJ\n";
    let view = DiffView::between(old, new, 1);
    assert_eq!(view.hunks.len(), 2);
    assert_eq!(
        (view.hunks[0].skipped_before, view.hunks[1].skipped_before),
        (0, 6)
    );
    assert_eq!(view.skipped_after, 0);
}

#[test]
fn pure_additions_and_removals_are_not_emphasized() {
    let view = DiffView::between("a\nb\n", "a\nx\nb\n", 0);
    assert_eq!(show(&view), "[skip 1]\n+ -/2 x\n[skip 1]");
    let view = DiffView::between("a\nx\nb\n", "a\nb\n", 0);
    assert_eq!(show(&view), "[skip 1]\n- 2/- x\n[skip 1]");
}

#[test]
fn side_by_side_rows_pair_the_i_th_removed_and_added_lines() {
    let view = DiffView::between("a\nold1\nold2\nold3\nz\n", "a\nnew1\nnew2\nz\n", 0);
    let block = &view.hunks[0].blocks[0];
    let rows: Vec<(Option<String>, Option<String>)> = block
        .rows()
        .into_iter()
        .map(|(l, r)| (l.map(Line::text), r.map(Line::text)))
        .collect();
    assert_eq!(
        rows,
        [
            (Some("old1".into()), Some("new1".into())),
            (Some("old2".into()), Some("new2".into())),
            (Some("old3".into()), None),
        ]
    );
}

#[test]
fn unchanged_lines_are_on_both_sides_of_a_row() {
    let view = DiffView::between("a\nb\n", "a\nB\n", 1);
    let rows = view.hunks[0].blocks[0].rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, rows[0].1);
}

#[test]
fn line_endings_are_dropped_and_a_missing_final_newline_is_fine() {
    let view = DiffView::between("a\r\nb", "a\r\nc", 0);
    assert_eq!(show(&view), "[skip 1]\n- 2/- b\n+ -/2 c\n[skip 0]");
}

#[test]
fn a_very_long_line_is_cut_off_and_the_view_says_so() {
    let long = "x".repeat(MAX_LINE_CHARS + 500);
    let view = DiffView::between("a\n", &format!("{long}\n"), 0);
    assert!(view.truncated);
    let Block::Changed { added, .. } = &view.hunks[0].blocks[0] else {
        panic!("expected a change");
    };
    assert_eq!(added[0].text().chars().count(), MAX_LINE_CHARS + 1);
    assert!(added[0].text().ends_with('…'));
}

#[test]
fn a_huge_diff_stops_at_the_line_limit_and_still_counts_everything() {
    let old: String = (0..MAX_SHOWN_LINES + 100)
        .map(|i| format!("old {i}\n"))
        .collect();
    let new: String = (0..MAX_SHOWN_LINES + 100)
        .map(|i| format!("new {i}\n"))
        .collect();
    let view = DiffView::between(&old, &new, 3);
    assert!(view.truncated);
    assert_eq!(
        (view.added, view.removed),
        (MAX_SHOWN_LINES + 100, MAX_SHOWN_LINES + 100)
    );
    let shown: usize = view
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.blocks)
        .map(|block| match block {
            Block::Same(lines) => lines.len(),
            Block::Changed { removed, added } => removed.len() + added.len(),
        })
        .sum();
    assert_eq!(shown, MAX_SHOWN_LINES);
    assert_eq!(view.skipped_after, 0);
}

#[test]
fn markup_in_text_is_kept_as_is_for_the_caller_to_escape() {
    let view = DiffView::between("<b>x</b>\n", "<b>y</b>\n", 0);
    let Block::Changed { removed, .. } = &view.hunks[0].blocks[0] else {
        panic!("expected a change");
    };
    assert!(removed[0].text().starts_with("<b>"));
}
