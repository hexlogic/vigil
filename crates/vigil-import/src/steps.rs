//! Checking the steps of a pipeline that a format has built.

use vigil_core::{Pipeline, Processor, Step};

use crate::report::Note;

/// The most characters of a library message that go into a note.
const REASON_CHARS: usize = 120;

/// The first line of a library message, short enough for a note.
pub(crate) fn reason(message: &str) -> String {
    message
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(REASON_CHARS)
        .collect()
}

/// Keep the steps Vigil can build, dropping each that cannot, with a note. A watch with one bad
/// filter keeps the others.
pub(crate) fn validated(
    processor: Processor,
    steps: Vec<Step>,
    notes: &mut Vec<Note>,
) -> Vec<Step> {
    let mut kept: Vec<Step> = Vec::new();
    for step in steps {
        kept.push(step);
        if let Err(error) = Pipeline::new(processor, &kept)
            && let Some(step) = kept.pop()
        {
            notes.push(Note::skipped(
                step.name(),
                format!(
                    "the {} filter was left out because it does not work here: {}",
                    step.name(),
                    reason(&error.to_string())
                ),
            ));
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use vigil_core::{Processor, SelectMode, Step};

    use super::{reason, validated};

    #[test]
    fn a_step_that_does_not_build_is_dropped_and_the_rest_kept() {
        let mut notes = Vec::new();
        let steps = vec![
            Step::Css {
                selector: "a[".into(),
                mode: SelectMode::Include,
                required: false,
            },
            Step::Trim {},
        ];
        assert_eq!(
            validated(Processor::Text, steps, &mut notes),
            [Step::Trim {}]
        );
        assert_eq!(notes.len(), 1);
        assert!(notes[0].message.contains("left out"), "{notes:?}");
    }

    #[test]
    fn a_reason_is_one_short_line() {
        assert_eq!(reason("first\nsecond"), "first");
        assert_eq!(reason(&"x".repeat(500)).chars().count(), 120);
    }
}
