//! A watch's processing pipeline: decode, process, then the configured steps.
//!
//! [`Pipeline::new`] checks and compiles the steps once; [`Pipeline::render`] turns a fetched
//! body into the text that is hashed, stored and diffed. Both a check and a preview call
//! `render`, so a preview shows exactly what a check would store.

mod error;
mod step;
mod text;

pub use error::{PipelineError, PipelineErrorKind};
pub use step::Step;

use crate::decode::decode;
use crate::model::Processor;
use crate::normalize::normalize;
use crate::process::{MAX_OUTPUT_BYTES, process};
use text::TextStep;

/// A compiled step with the name used to report errors about it.
#[derive(Debug)]
struct Slot {
    name: &'static str,
    step: Compiled,
}

#[derive(Debug)]
enum Compiled {
    Text(TextStep),
}

impl Compiled {
    fn apply(&self, text: &str) -> Result<String, PipelineErrorKind> {
        match self {
            Self::Text(step) => step.apply(text),
        }
    }
}

/// A processor and its steps, checked and ready to run.
#[derive(Debug)]
pub struct Pipeline {
    processor: Processor,
    steps: Vec<Slot>,
}

impl Pipeline {
    /// Check `steps` against `processor` and compile them. Invalid regular expressions,
    /// patterns and capture groups are reported here, naming the step.
    pub fn new(processor: Processor, steps: &[Step]) -> Result<Self, PipelineError> {
        let mut slots = Vec::with_capacity(steps.len());
        for (index, step) in steps.iter().enumerate() {
            let name = step.name();
            let compiled = compile(step).map_err(|kind| PipelineError::at(index, name, kind))?;
            slots.push(Slot {
                name,
                step: compiled,
            });
        }
        Ok(Self {
            processor,
            steps: slots,
        })
    }

    /// Turn a fetched body into its final text.
    ///
    /// The body is decoded (see [`decode`]), run through the processor, normalised, and then
    /// through every step in order, normalising again after each. The result ends in exactly
    /// one newline, or is empty.
    pub fn render(
        &self,
        bytes: &[u8],
        content_type: Option<&str>,
    ) -> Result<String, PipelineError> {
        let decoded = decode(bytes, content_type);
        let mut text = normalize(&process(self.processor, &decoded, content_type)?);
        if text.len() > MAX_OUTPUT_BYTES {
            return Err(PipelineErrorKind::TooLarge.into());
        }
        for (index, slot) in self.steps.iter().enumerate() {
            let output = slot
                .step
                .apply(&text)
                .map_err(|kind| PipelineError::at(index, slot.name, kind))?;
            text = normalize(&output);
        }
        Ok(text)
    }
}

fn compile(step: &Step) -> Result<Compiled, PipelineErrorKind> {
    let text = match step {
        Step::RegexExtract { pattern, group } => text::extract(pattern, *group)?,
        Step::RegexReplace {
            pattern,
            replacement,
        } => text::replace(pattern, replacement)?,
        Step::IgnoreLines { patterns } => text::ignore(patterns)?,
        Step::Sort {} => TextStep::Sort,
        Step::Unique {} => TextStep::Unique,
        Step::Trim {} => TextStep::Trim,
        Step::CollapseWhitespace {} => TextStep::CollapseWhitespace,
        Step::Lowercase {} => TextStep::Lowercase,
    };
    Ok(Compiled::Text(text))
}

impl From<PipelineErrorKind> for PipelineError {
    fn from(kind: PipelineErrorKind) -> Self {
        Self::general(kind)
    }
}

#[cfg(test)]
mod tests {
    use super::{Pipeline, PipelineErrorKind, Step};
    use crate::model::Processor;

    #[test]
    fn errors_about_a_step_carry_its_number_and_name() {
        let steps = [
            Step::Trim {},
            Step::RegexExtract {
                pattern: "a(b)".into(),
                group: 3,
            },
        ];
        let Err(error) = Pipeline::new(Processor::Text, &steps) else {
            panic!("a missing capture group must be rejected");
        };
        assert_eq!(error.step(), Some(2));
        assert!(matches!(
            error.kind(),
            PipelineErrorKind::NoSuchGroup {
                group: 3,
                groups: 1
            }
        ));
        assert!(error.to_string().starts_with("step 2 (regex_extract): "));
    }

    #[test]
    fn errors_about_the_body_have_no_step() -> Result<(), Box<dyn std::error::Error>> {
        let pipeline = Pipeline::new(Processor::Json, &[Step::Sort {}])?;
        let Err(error) = pipeline.render(b"not json", None) else {
            panic!("a body that is not JSON must be rejected");
        };
        assert_eq!(error.step(), None);
        assert!(
            error
                .to_string()
                .starts_with("the response is not valid JSON: "),
            "{error}"
        );
        Ok(())
    }

    #[test]
    fn the_output_is_normalised() -> Result<(), Box<dyn std::error::Error>> {
        let pipeline = Pipeline::new(Processor::Text, &[Step::Lowercase {}])?;
        assert_eq!(
            pipeline.render(
                b"\r\n\r\n  Hello  \r\n\r\n\r\nWorld\r\n\r\n",
                Some("text/plain")
            )?,
            "  hello\n\nworld\n"
        );
        assert_eq!(pipeline.render(b"", None)?, "");
        Ok(())
    }

    #[test]
    fn bodies_are_decoded_before_processing() -> Result<(), Box<dyn std::error::Error>> {
        let pipeline = Pipeline::new(Processor::Text, &[])?;
        let latin1 = [0x63, 0x61, 0x66, 0xe9];
        assert_eq!(
            pipeline.render(&latin1, Some("text/plain; charset=iso-8859-1"))?,
            "café\n"
        );
        Ok(())
    }
}
