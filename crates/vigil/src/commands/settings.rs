//! Turning the content options of `watch add|edit` into watch settings.

use anyhow::{Context, anyhow, bail};
use serde::de::DeserializeOwned;
use vigil_core::{Pipeline, PipelineErrorKind, Watch};
use vigil_store::NewWatch;

use crate::cli::ContentArgs;

/// Parse the value of a `--pipeline` or `--conditions` option: JSON, or `@path` to a file
/// that holds it.
pub fn read_json<T: DeserializeOwned>(flag: &str, value: &str) -> anyhow::Result<T> {
    let text = match value.strip_prefix('@') {
        Some(path) => {
            std::fs::read_to_string(path).with_context(|| format!("reading {path} for --{flag}"))?
        }
        None => value.to_owned(),
    };
    serde_json::from_str(&text).with_context(|| format!("invalid --{flag}"))
}

/// Apply the options that were given to `settings`, then check that the result works
/// together: the steps fit the processor and every selector, regex and pattern compiles.
pub fn apply(settings: &mut NewWatch, args: &ContentArgs) -> anyhow::Result<()> {
    if let Some(processor) = args.processor {
        settings.processor = processor;
    }
    if let Some(value) = &args.pipeline {
        settings.pipeline = read_json("pipeline", value)?;
    }
    if let Some(value) = &args.conditions {
        settings.conditions = read_json("conditions", value)?;
    }
    validate(settings)
}

/// A copy of `watch` with the options that were given applied, for trying settings out
/// without saving them.
pub fn overridden(watch: &Watch, args: &ContentArgs) -> anyhow::Result<Watch> {
    let mut settings = NewWatch::from(watch);
    apply(&mut settings, args)?;
    Ok(Watch {
        processor: settings.processor,
        pipeline: settings.pipeline,
        conditions: settings.conditions,
        ..watch.clone()
    })
}

/// Check a watch's processor, steps and conditions without fetching anything.
pub fn validate(settings: &NewWatch) -> anyhow::Result<()> {
    if let Err(error) = Pipeline::new(settings.processor, &settings.pipeline) {
        let hint = match error.kind() {
            PipelineErrorKind::WrongProcessor { .. } | PipelineErrorKind::OutOfOrder { .. } => {
                "\nthe steps and the processor are set separately: change the processor with \
                 --processor, or replace the steps with --pipeline ('[]' removes them)"
            }
            _ => "",
        };
        bail!("invalid pipeline: {error}{hint}");
    }
    settings
        .conditions
        .compile()
        .map_err(|error| anyhow!("invalid conditions: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use vigil_core::{Processor, Schedule, SelectMode, Step};

    use super::*;

    fn fresh() -> NewWatch {
        NewWatch::new("https://example.com", Schedule::Interval(60))
    }

    fn args(
        processor: Option<Processor>,
        pipeline: Option<&str>,
        conditions: Option<&str>,
    ) -> ContentArgs {
        ContentArgs {
            processor,
            pipeline: pipeline.map(str::to_owned),
            conditions: conditions.map(str::to_owned),
        }
    }

    fn message(result: anyhow::Result<()>) -> String {
        result.err().map(|e| format!("{e:#}")).unwrap_or_default()
    }

    #[test]
    fn options_that_were_not_given_change_nothing() -> anyhow::Result<()> {
        let mut settings = fresh();
        settings.processor = Processor::Json;
        settings.pipeline = vec![Step::Sort {}];
        let before = settings.clone();
        apply(&mut settings, &ContentArgs::default())?;
        assert_eq!(settings, before);
        Ok(())
    }

    #[test]
    fn pipeline_and_conditions_come_from_json() -> anyhow::Result<()> {
        let mut settings = fresh();
        apply(
            &mut settings,
            &args(
                None,
                Some(r##"[{"type": "css", "selector": "#price"}, {"type": "trim"}]"##),
                Some(r#"{"trigger_text": ["in stock"]}"#),
            ),
        )?;
        assert_eq!(
            settings.pipeline,
            [
                Step::Css {
                    selector: "#price".into(),
                    mode: SelectMode::Include,
                    required: false
                },
                Step::Trim {}
            ]
        );
        assert_eq!(settings.conditions.trigger_text, ["in stock"]);
        Ok(())
    }

    #[test]
    fn json_can_come_from_a_file() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("steps.json");
        std::fs::write(&path, r#"[{"type": "lowercase"}]"#)?;
        let mut settings = fresh();
        apply(
            &mut settings,
            &args(None, Some(&format!("@{}", path.display())), None),
        )?;
        assert_eq!(settings.pipeline, [Step::Lowercase {}]);
        Ok(())
    }

    #[test]
    fn a_missing_file_is_named() {
        let text = message(apply(
            &mut fresh(),
            &args(None, Some("@/nonexistent/steps.json"), None),
        ));
        assert!(
            text.starts_with("reading /nonexistent/steps.json for --pipeline"),
            "{text}"
        );
    }

    #[test]
    fn bad_json_names_the_option() {
        let text = message(apply(
            &mut fresh(),
            &args(None, Some("[{\"type\": \"nope\"}]"), None),
        ));
        assert!(
            text.starts_with("invalid --pipeline: unknown variant `nope`"),
            "{text}"
        );
        let text = message(apply(
            &mut fresh(),
            &args(None, None, Some("{\"trigger\": []}")),
        ));
        assert!(
            text.starts_with("invalid --conditions: unknown field `trigger`"),
            "{text}"
        );
        let text = message(apply(&mut fresh(), &args(None, Some("not json"), None)));
        assert!(text.starts_with("invalid --pipeline: "), "{text}");
    }

    #[test]
    fn a_step_that_does_not_compile_is_refused_with_its_number() {
        let text = message(apply(
            &mut fresh(),
            &args(
                None,
                Some(
                    r#"[{"type": "trim"}, {"type": "regex_replace", "pattern": "(", "replacement": ""}]"#,
                ),
                None,
            ),
        ));
        assert!(
            text.starts_with("invalid pipeline: step 2 (regex_replace): invalid regex"),
            "{text}"
        );
    }

    #[test]
    fn a_pattern_that_does_not_compile_is_refused() {
        let text = message(apply(
            &mut fresh(),
            &args(None, None, Some(r#"{"block_text": ["/(/"]}"#)),
        ));
        assert!(
            text.starts_with("invalid conditions: block_text[0]: invalid regex"),
            "{text}"
        );
    }

    #[test]
    fn steps_that_do_not_fit_the_processor_come_with_a_hint() -> anyhow::Result<()> {
        let mut settings = fresh();
        apply(
            &mut settings,
            &args(None, Some(r#"[{"type": "css", "selector": "p"}]"#), None),
        )?;
        let text = message(apply(
            &mut settings,
            &args(Some(Processor::Json), None, None),
        ));
        assert!(
            text.starts_with("invalid pipeline: step 1 (css): html steps need the text processor"),
            "{text}"
        );
        assert!(text.contains("--pipeline ('[]' removes them)"), "{text}");
        // Changing both at once is fine.
        apply(
            &mut settings,
            &args(Some(Processor::Json), Some("[]"), None),
        )?;
        assert_eq!(settings.processor, Processor::Json);
        Ok(())
    }
}
