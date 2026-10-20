//! A watch's processing pipeline: decode, process, then the configured steps.
//!
//! [`Pipeline::new`] checks and compiles the steps once; [`Pipeline::render`] turns a fetched
//! body into the text that is hashed, stored and diffed. Both a check and a preview call
//! `render`, so a preview shows exactly what a check would store.

mod error;
mod html;
mod jq;
mod json;
mod step;
mod text;
mod xpath;

pub use error::{PipelineError, PipelineErrorKind};
pub use step::{SelectMode, Step};

use crate::decode::decode;
use crate::model::Processor;
use crate::normalize::normalize;
use crate::process::{
    MAX_OUTPUT_BYTES, check_html_depth, parse_json, process, process_text, render_json,
};
use html::HtmlStep;
use json::JsonStep;
use step::Stage;
use text::TextStep;

/// A compiled step with its position and the name used to report errors about it.
#[derive(Debug)]
struct Slot<T> {
    index: usize,
    name: &'static str,
    step: T,
}

impl<T> Slot<T> {
    fn fail(&self, kind: PipelineErrorKind) -> PipelineError {
        PipelineError::at(self.index, self.name, kind)
    }
}

/// A processor and its steps, checked and ready to run.
#[derive(Debug)]
pub struct Pipeline {
    processor: Processor,
    html: Vec<Slot<HtmlStep>>,
    json: Vec<Slot<JsonStep>>,
    text: Vec<Slot<TextStep>>,
}

impl Pipeline {
    /// Check `steps` against `processor` and compile them. Invalid selectors, regular
    /// expressions, patterns and capture groups, steps that need another processor, and steps
    /// in the wrong order are reported here, naming the step.
    ///
    /// Steps must be ordered by what they work on: HTML steps first, then JSON steps, then
    /// text steps.
    pub fn new(processor: Processor, steps: &[Step]) -> Result<Self, PipelineError> {
        let mut html = Vec::new();
        let mut json = Vec::new();
        let mut text = Vec::new();
        let mut stage = Stage::Html;
        for (index, step) in steps.iter().enumerate() {
            let name = step.name();
            let fail = |kind: PipelineErrorKind| PipelineError::at(index, name, kind);
            if step.stage() < stage {
                return Err(fail(PipelineErrorKind::OutOfOrder {
                    stage: step.stage().label(),
                    after: stage.label(),
                }));
            }
            stage = step.stage();
            let needed = match step.stage() {
                Stage::Html => Some(Processor::Text),
                Stage::Json => Some(Processor::Json),
                Stage::Text => None,
            };
            if let Some(needed) = needed
                && processor != needed
            {
                return Err(fail(PipelineErrorKind::WrongProcessor {
                    stage: step.stage().label(),
                    needed,
                    found: processor,
                }));
            }
            match step {
                Step::Css {
                    selector,
                    mode,
                    required,
                } => html.push(Slot {
                    index,
                    name,
                    step: html::css(selector, *mode, *required).map_err(fail)?,
                }),
                Step::XPath {
                    expr,
                    mode,
                    required,
                } => html.push(Slot {
                    index,
                    name,
                    step: xpath::xpath(expr, *mode, *required).map_err(fail)?,
                }),
                Step::JsonPath { expr, required } => json.push(Slot {
                    index,
                    name,
                    step: json::jsonpath(expr, *required).map_err(fail)?,
                }),
                Step::Jq { program } => json.push(Slot {
                    index,
                    name,
                    step: json::jq(program).map_err(fail)?,
                }),
                Step::RegexExtract {
                    pattern,
                    group,
                    required,
                } => text.push(Slot {
                    index,
                    name,
                    step: text::extract(pattern, *group, *required).map_err(fail)?,
                }),
                Step::RegexReplace {
                    pattern,
                    replacement,
                } => text.push(Slot {
                    index,
                    name,
                    step: text::replace(pattern, replacement).map_err(fail)?,
                }),
                Step::IgnoreLines { patterns } => text.push(Slot {
                    index,
                    name,
                    step: text::ignore(patterns).map_err(fail)?,
                }),
                Step::Sort {} => text.push(Slot {
                    index,
                    name,
                    step: TextStep::Sort,
                }),
                Step::Unique {} => text.push(Slot {
                    index,
                    name,
                    step: TextStep::Unique,
                }),
                Step::Trim {} => text.push(Slot {
                    index,
                    name,
                    step: TextStep::Trim,
                }),
                Step::CollapseWhitespace {} => text.push(Slot {
                    index,
                    name,
                    step: TextStep::CollapseWhitespace,
                }),
                Step::Lowercase {} => text.push(Slot {
                    index,
                    name,
                    step: TextStep::Lowercase,
                }),
            }
        }
        Ok(Self {
            processor,
            html,
            json,
            text,
        })
    }

    /// Turn a fetched body into its final text.
    ///
    /// The body is decoded (see [`decode`]) and turned into text by the processor. With HTML
    /// steps, the processor is bypassed: the page is parsed as HTML whatever its content type
    /// says, the steps are applied to the tree, and the result is converted to text. With JSON
    /// steps, the body is parsed, the steps are applied to the value, and it is printed like
    /// the `json` processor does. The text
    /// is normalised, then run through every text step in order, normalising after each. The
    /// result ends in exactly one newline, or is empty.
    pub fn render(
        &self,
        bytes: &[u8],
        content_type: Option<&str>,
    ) -> Result<String, PipelineError> {
        let decoded = decode(bytes, content_type);
        let processed = if !self.html.is_empty() {
            check_html_depth(&decoded)?;
            let mut document = html::parse(&decoded);
            for slot in &self.html {
                document = slot.step.apply(document).map_err(|kind| slot.fail(kind))?;
            }
            process_text(&html::serialize(&document), Some("text/html"))?
        } else if !self.json.is_empty() {
            let mut value = parse_json(&decoded)?;
            for slot in &self.json {
                value = slot.step.apply(value).map_err(|kind| slot.fail(kind))?;
            }
            render_json(&value)?
        } else {
            process(self.processor, &decoded, content_type)?
        };
        let mut text = normalize(&processed);
        if text.len() > MAX_OUTPUT_BYTES {
            return Err(PipelineErrorKind::TooLarge.into());
        }
        for slot in &self.text {
            let output = slot.step.apply(&text).map_err(|kind| slot.fail(kind))?;
            text = normalize(&output);
        }
        Ok(text)
    }
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
                required: false,
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
