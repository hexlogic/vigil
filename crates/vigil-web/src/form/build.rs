//! Checking what was typed, and turning it into a watch.

use std::collections::{BTreeMap, HashMap};

use uuid::Uuid;
use vigil_core::{
    Conditions, FetchSettings, NotifyOn, Processor, Schedule, SelectMode, Step, Watch, WatchId,
    WatchState,
};
use vigil_notify::validate_template;
use vigil_scheduler::{Checker, IntervalError, parse_interval};
use vigil_store::{NewWatch, NotificationTarget, Tag};

use super::draft::{Draft, StepDraft};
use crate::i18n::Translator;

/// Interval when neither the form nor a tag says.
const DEFAULT_EVERY_SECS: u64 = 3600;

/// The tags and targets that exist, which the form may choose from.
pub(crate) struct Known<'a> {
    pub(crate) tags: &'a [Tag],
    pub(crate) targets: &'a [NotificationTarget],
}

/// A form that passed every check.
#[derive(Debug)]
pub(crate) struct Built {
    pub(crate) new: NewWatch,
    pub(crate) template: Option<String>,
    pub(crate) tags: Vec<Uuid>,
    pub(crate) targets: Vec<Uuid>,
}

/// What is wrong with a form, by where it is shown.
#[derive(Debug, Default)]
pub(crate) struct Problems {
    pub(crate) url: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) every: Option<String>,
    pub(crate) jitter: Option<String>,
    pub(crate) conditions: Option<String>,
    pub(crate) timeout: Option<String>,
    pub(crate) headers: Option<String>,
    pub(crate) retention: Option<String>,
    pub(crate) template: Option<String>,
    pub(crate) tags: Option<String>,
    pub(crate) targets: Option<String>,
    /// Problems with the filters as a whole, such as ones in the wrong order.
    pub(crate) pipeline: Option<String>,
    steps: HashMap<usize, String>,
}

impl Problems {
    /// What is wrong with the filter at `at`, if anything.
    pub(crate) fn step(&self, at: usize) -> Option<&str> {
        self.steps.get(&at).map(String::as_str)
    }

    /// Whether the form has no problem.
    pub(crate) fn is_empty(&self) -> bool {
        let fields = [
            &self.url,
            &self.name,
            &self.every,
            &self.jitter,
            &self.conditions,
            &self.timeout,
            &self.headers,
            &self.retention,
            &self.template,
            &self.tags,
            &self.targets,
            &self.pipeline,
        ];
        self.steps.is_empty() && fields.iter().all(|field| field.is_none())
    }
}

/// Check a form and build the watch it describes. Library diagnostics (a bad selector, a bad
/// pattern) are shown as they are, in English; everything the web crate says itself is
/// translated.
pub(crate) async fn build(
    draft: &Draft,
    t: &Translator,
    known: &Known<'_>,
    checker: &Checker,
) -> Result<Built, Box<Problems>> {
    let mut problems = Problems::default();

    let url = draft.url.trim().to_owned();
    if url.is_empty() {
        problems.url = Some(t.t("form.err.url_required"));
    } else if let Err(error) = vigil_fetch::check_url(&url) {
        problems.url = Some(vigil_notify::describe_error(&error));
    }
    let name = match draft.name.trim() {
        "" => url.clone(),
        name => name.to_owned(),
    };

    let tags = chosen(
        &draft.tags,
        known.tags.iter().map(|tag| tag.id),
        &mut problems.tags,
        t,
    );
    let targets = chosen(
        &draft.targets,
        known.targets.iter().map(|target| target.id),
        &mut problems.targets,
        t,
    );

    let every = every(draft, known.tags, &tags, t, &mut problems);
    let jitter = number(&draft.jitter, 0, &mut problems.jitter, t);
    let retention = match draft.retention.trim() {
        "" => 100,
        text => match text.parse::<u32>() {
            Ok(n) if n >= 1 => n,
            _ => {
                problems.retention = Some(t.t("form.err.retention"));
                100
            }
        },
    };
    let fetch = fetch_settings(draft, t, &mut problems);
    let processor = draft
        .processor
        .parse::<Processor>()
        .unwrap_or(Processor::Text);

    let mut pipeline = Vec::new();
    for (at, step) in draft.steps.iter().enumerate() {
        match step_of(step) {
            Ok(step) => pipeline.push(step),
            Err(key) => {
                problems.steps.insert(at, t.t(key));
            }
        }
    }

    let conditions = Conditions {
        trigger_text: lines(&draft.trigger_text),
        block_text: lines(&draft.block_text),
        notify_on: NotifyOn {
            added: draft.notify_added,
            removed: draft.notify_removed,
            changed: draft.notify_changed,
        },
    };
    if !conditions.notify_on.added && !conditions.notify_on.removed && !conditions.notify_on.changed
    {
        problems.conditions = Some(t.t("form.err.notify_on"));
    }

    let template = match draft.template.trim() {
        "" => None,
        text => {
            if let Err(error) = validate_template(text) {
                problems.template = Some(error.to_string());
            }
            Some(text.to_owned())
        }
    };

    let new = NewWatch {
        name,
        url,
        fetch,
        schedule: Schedule::Interval(every),
        jitter_secs: jitter,
        processor,
        pipeline,
        conditions,
        snapshot_retention: retention,
    };

    // The filters and conditions are checked together, as a check would build them, but only
    // when each filter at least has what it needs.
    if problems.steps.is_empty() {
        let watch = watch_of(&new, super::slot_of(&draft.id));
        if let Err(error) = checker.validate_watch(&watch, watch.id).await {
            match error.step {
                Some(number) => {
                    problems.steps.insert(number - 1, error.message);
                }
                None if error.message.starts_with("conditions:") => {
                    problems.conditions = Some(error.message);
                }
                None => problems.pipeline = Some(error.message),
            }
        }
    }

    if problems.is_empty() {
        Ok(Built {
            new,
            template,
            tags,
            targets,
        })
    } else {
        Err(Box::new(problems))
    }
}

/// A watch to validate: the form's settings under a slot of the form's own.
fn watch_of(new: &NewWatch, id: WatchId) -> Watch {
    Watch {
        id,
        name: new.name.clone(),
        url: new.url.clone(),
        paused: false,
        fetch: new.fetch.clone(),
        schedule: new.schedule,
        jitter_secs: new.jitter_secs,
        processor: new.processor,
        pipeline: new.pipeline.clone(),
        conditions: new.conditions.clone(),
        snapshot_retention: new.snapshot_retention,
        notification_template: None,
        created_at: 0,
        updated_at: 0,
        state: WatchState::default(),
    }
}

/// The ids in `chosen` that exist among `known`; anything else is a problem.
fn chosen(
    chosen: &[String],
    known: impl Iterator<Item = Uuid>,
    problem: &mut Option<String>,
    t: &Translator,
) -> Vec<Uuid> {
    let known: Vec<Uuid> = known.collect();
    let mut out = Vec::new();
    for text in chosen {
        match text.parse::<Uuid>() {
            Ok(id) if known.contains(&id) => {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
            _ => *problem = Some(t.t("form.err.unknown_choice")),
        }
    }
    out
}

/// The interval: what was typed, else the shortest default of the chosen tags, else an hour.
fn every(
    draft: &Draft,
    tags: &[Tag],
    chosen: &[Uuid],
    t: &Translator,
    problems: &mut Problems,
) -> u64 {
    let text = draft.every.trim();
    if text.is_empty() {
        return tags
            .iter()
            .filter(|tag| chosen.contains(&tag.id))
            .filter_map(|tag| tag.default_interval_secs)
            .min()
            .unwrap_or(DEFAULT_EVERY_SECS);
    }
    match parse_interval(text) {
        Ok(every) => every.as_secs(),
        Err(error) => {
            problems.every = Some(t.t(match error {
                IntervalError::Invalid(_) => "form.err.every_invalid",
                IntervalError::TooShort => "form.err.every_short",
                IntervalError::TooLong => "form.err.every_long",
            }));
            DEFAULT_EVERY_SECS
        }
    }
}

/// A whole number typed in a field, or `default` when it is empty.
fn number(text: &str, default: u32, problem: &mut Option<String>, t: &Translator) -> u32 {
    match text.trim() {
        "" => default,
        text => text.parse().unwrap_or_else(|_| {
            *problem = Some(t.t("form.err.number"));
            default
        }),
    }
}

fn fetch_settings(draft: &Draft, t: &Translator, problems: &mut Problems) -> FetchSettings {
    let timeout_secs = match draft.timeout.trim() {
        "" => None,
        text => match text.parse::<u64>() {
            Ok(n) if (1..=3600).contains(&n) => Some(n),
            _ => {
                problems.timeout = Some(t.t("form.err.timeout"));
                None
            }
        },
    };
    let mut headers = BTreeMap::new();
    for (number, line) in draft.headers.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parsed = line.split_once(':').map(|(n, v)| (n.trim(), v.trim()));
        match parsed {
            Some((name, value))
                if !name.is_empty()
                    && name.bytes().all(|b| b.is_ascii_graphic())
                    && !value.chars().any(char::is_control) =>
            {
                headers.insert(name.to_owned(), value.to_owned());
            }
            _ => {
                problems.headers = Some(t.t1("form.err.header", "n", number + 1));
                break;
            }
        }
    }
    let user_agent = Some(draft.user_agent.trim().to_owned()).filter(|ua| !ua.is_empty());
    FetchSettings {
        timeout_secs,
        user_agent,
        headers,
    }
}

/// The lines of a text area, without blanks.
fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A filter from its fields; the key of the problem if it lacks one.
fn step_of(step: &StepDraft) -> Result<Step, &'static str> {
    let need = |text: &str| -> Result<String, &'static str> {
        if text.trim().is_empty() {
            Err("form.err.step_empty")
        } else {
            Ok(text.trim().to_owned())
        }
    };
    let mode = if step.mode == "remove" {
        SelectMode::Remove
    } else {
        SelectMode::Include
    };
    Ok(match step.kind.as_str() {
        "css" => Step::Css {
            selector: need(&step.selector)?,
            mode,
            required: step.required,
        },
        "xpath" => Step::XPath {
            expr: need(&step.expr)?,
            mode,
            required: step.required,
        },
        "jsonpath" => Step::JsonPath {
            expr: need(&step.expr)?,
            required: step.required,
        },
        "jq" => Step::Jq {
            program: need(&step.program)?,
        },
        "regex_extract" => Step::RegexExtract {
            pattern: need(&step.pattern)?,
            group: match step.group.trim() {
                "" => 0,
                text => text.parse().map_err(|_| "form.err.step_group")?,
            },
            required: step.required,
        },
        "regex_replace" => Step::RegexReplace {
            pattern: need(&step.pattern)?,
            // An empty replacement deletes the matches, which is a fine thing to want.
            replacement: step.replacement.clone(),
        },
        "ignore_lines" => {
            let patterns = lines(&step.patterns);
            if patterns.is_empty() {
                return Err("form.err.step_empty");
            }
            Step::IgnoreLines { patterns }
        }
        "sort" => Step::Sort {},
        "unique" => Step::Unique {},
        "trim" => Step::Trim {},
        "collapse_whitespace" => Step::CollapseWhitespace {},
        "lowercase" => Step::Lowercase {},
        _ => return Err("form.err.step_kind"),
    })
}
