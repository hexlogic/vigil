use super::{Filters, map_filters, pattern};
use vigil_core::{Processor, SelectMode, Step};

fn pat(entry: &str) -> (Option<String>, usize) {
    let mut notes = Vec::new();
    (pattern(entry, "ignore_text", &mut notes), notes.len())
}

#[test]
fn a_regular_expression_without_flags_is_case_insensitive_as_in_the_source() {
    assert_eq!(pat("/price: \\d+/").0.as_deref(), Some("/price: \\d+/i"));
    assert_eq!(pat("/Sold Out/s").0.as_deref(), Some("/Sold Out/s"));
    assert_eq!(pat("/x/gi").0.as_deref(), Some("/x/i"));
    assert_eq!(pat("plain text").0.as_deref(), Some("plain text"));
    // A flag that Vigil does not have is dropped, and said.
    assert_eq!(pat("/x/a"), (Some("/x/".to_owned()), 1));
    // A Python-only construct does not build here: the pattern is dropped, and said.
    assert_eq!(pat("/(?<=a)b/"), (None, 1));
}

#[test]
fn includes_are_told_apart_by_their_first_characters() {
    let filters = Filters {
        include: vec![".price".into(), "div.x".into()],
        subtractive: vec![".ad".into(), "xpath://footer".into()],
        ..Filters::default()
    };
    let mapped = map_filters(&filters);
    assert_eq!(mapped.processor, Processor::Text);
    assert_eq!(
        mapped.pipeline,
        [
            Step::Css {
                selector: ".price, div.x".into(),
                mode: SelectMode::Include,
                required: false
            },
            Step::Css {
                selector: ".ad".into(),
                mode: SelectMode::Remove,
                required: false
            },
            Step::XPath {
                expr: "//footer".into(),
                mode: SelectMode::Remove,
                required: false
            },
        ]
    );
    assert_eq!(mapped.notes.len(), 1, "{:?}", mapped.notes);

    let json = map_filters(&Filters {
        include: vec!["jqraw:.data[].title".into()],
        ..Filters::default()
    });
    assert_eq!(json.processor, Processor::Json);
    assert_eq!(
        json.pipeline,
        [Step::Jq {
            program: ".data[].title".into(),
            raw: true
        }]
    );
    assert!(json.notes.is_empty());
}

#[test]
fn a_filter_that_does_not_build_is_dropped_alone() {
    let mapped = map_filters(&Filters {
        include: vec!["a[".into()],
        trim: true,
        ..Filters::default()
    });
    assert_eq!(mapped.pipeline, [Step::Trim {}]);
    assert_eq!(mapped.notes.len(), 1);
    assert!(
        mapped.notes[0].message.contains("left out"),
        "{:?}",
        mapped.notes
    );
}

#[test]
fn extracts_are_one_pattern_with_the_groups_the_source_would_return() {
    let one = map_filters(&Filters {
        extract: vec!["/Price: (\\d+)/".into()],
        ..Filters::default()
    });
    assert_eq!(
        one.pipeline,
        [Step::RegexExtract {
            pattern: "(?i)Price: (\\d+)".into(),
            group: 1,
            required: false
        }]
    );
    let plain = map_filters(&Filters {
        extract: vec!["a.b".into(), "/c+/".into()],
        ..Filters::default()
    });
    assert_eq!(
        plain.pipeline,
        [Step::RegexExtract {
            pattern: "(?:(?i)a\\.b)|(?:(?i)c+)".into(),
            group: 0,
            required: false
        }]
    );
    let mixed = map_filters(&Filters {
        extract: vec!["/(a)/".into(), "/b/".into()],
        ..Filters::default()
    });
    assert_eq!(mixed.pipeline.len(), 1);
    assert_eq!(mixed.notes.len(), 1);
}

#[test]
fn flags_that_are_all_dropped_leave_a_working_case_sensitive_expression() {
    // Written flags replace the case-insensitive default, as in the source, so `g` alone
    // leaves the expression case-sensitive. The empty flags group `(?)` would not build.
    let g = map_filters(&Filters {
        extract: vec!["/price: (\\d+)/g".into()],
        ..Filters::default()
    });
    assert_eq!(
        g.pipeline,
        [Step::RegexExtract {
            pattern: "price: (\\d+)".into(),
            group: 1,
            required: false
        }]
    );
    assert!(g.notes.is_empty(), "{:?}", g.notes);

    // A flag that Vigil does not have is said, and the rest still builds.
    let u = map_filters(&Filters {
        extract: vec!["/x/u".into()],
        ..Filters::default()
    });
    assert_eq!(
        u.pipeline,
        [Step::RegexExtract {
            pattern: "x".into(),
            group: 0,
            required: false
        }]
    );
    assert_eq!(u.notes.len(), 1, "{:?}", u.notes);
    assert!(u.notes[0].message.contains("flag"), "{:?}", u.notes);

    let lines = map_filters(&Filters {
        extract_lines: vec!["/Sold/g".into()],
        ..Filters::default()
    });
    assert_eq!(
        lines.pipeline,
        [Step::RegexExtract {
            pattern: "(?m)^.*(?:(?:Sold)).*$".into(),
            group: 0,
            required: false
        }]
    );
    assert!(lines.notes.is_empty(), "{:?}", lines.notes);

    // The same rule for the patterns of the conditions and of the ignored lines.
    assert_eq!(pat("/Sold/g"), (Some("/Sold/".to_owned()), 0));
}
