//! What is in the form, as typed: every value is still text, nothing is checked yet.

use vigil_core::{Scope, SelectMode, Step, Watch};
use vigil_store::{NotificationTarget, Tag};

use super::Fields;

/// The kinds of filter, in the order the picker lists them, with the key of each one's name.
pub(crate) const STEP_KINDS: [(&str, &str); 12] = [
    ("css", "form.kind.css"),
    ("xpath", "form.kind.xpath"),
    ("jsonpath", "form.kind.jsonpath"),
    ("jq", "form.kind.jq"),
    ("regex_extract", "form.kind.regex_extract"),
    ("regex_replace", "form.kind.regex_replace"),
    ("ignore_lines", "form.kind.ignore_lines"),
    ("sort", "form.kind.sort"),
    ("unique", "form.kind.unique"),
    ("trim", "form.kind.trim"),
    ("collapse_whitespace", "form.kind.collapse_whitespace"),
    ("lowercase", "form.kind.lowercase"),
];

/// One filter as typed. Which fields matter depends on the kind; the others stay empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StepDraft {
    pub(crate) kind: String,
    pub(crate) selector: String,
    pub(crate) expr: String,
    pub(crate) program: String,
    pub(crate) pattern: String,
    pub(crate) replacement: String,
    pub(crate) patterns: String,
    pub(crate) mode: String,
    pub(crate) group: String,
    pub(crate) required: bool,
}

impl StepDraft {
    /// A new filter of a kind, with its usual settings.
    pub(crate) fn new(kind: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            mode: "include".to_owned(),
            group: "0".to_owned(),
            ..Self::default()
        }
    }

    /// The filter at position `at` of a posted form.
    fn from_fields(fields: &Fields, at: usize) -> Self {
        let field = |name: &str| fields.get(&format!("step-{at}-{name}")).to_owned();
        Self {
            kind: field("type"),
            selector: field("selector"),
            expr: field("expr"),
            program: field("program"),
            pattern: field("pattern"),
            replacement: field("replacement"),
            patterns: field("patterns"),
            mode: field("mode"),
            group: field("group"),
            required: fields.has(&format!("step-{at}-required")),
        }
    }

    fn from_step(step: &Step) -> Self {
        let mode = |mode: &SelectMode| match mode {
            SelectMode::Include => "include",
            SelectMode::Remove => "remove",
        };
        match step {
            Step::Css {
                selector,
                mode: m,
                required,
            } => Self {
                selector: selector.clone(),
                mode: mode(m).to_owned(),
                required: *required,
                ..Self::new("css")
            },
            Step::XPath {
                expr,
                mode: m,
                required,
            } => Self {
                expr: expr.clone(),
                mode: mode(m).to_owned(),
                required: *required,
                ..Self::new("xpath")
            },
            Step::JsonPath { expr, required } => Self {
                expr: expr.clone(),
                required: *required,
                ..Self::new("jsonpath")
            },
            Step::Jq { program } => Self {
                program: program.clone(),
                ..Self::new("jq")
            },
            Step::RegexExtract {
                pattern,
                group,
                required,
            } => Self {
                pattern: pattern.clone(),
                group: group.to_string(),
                required: *required,
                ..Self::new("regex_extract")
            },
            Step::RegexReplace {
                pattern,
                replacement,
            } => Self {
                pattern: pattern.clone(),
                replacement: replacement.clone(),
                ..Self::new("regex_replace")
            },
            Step::IgnoreLines { patterns } => Self {
                patterns: patterns.join("\n"),
                ..Self::new("ignore_lines")
            },
            Step::Sort {} => Self::new("sort"),
            Step::Unique {} => Self::new("unique"),
            Step::Trim {} => Self::new("trim"),
            Step::CollapseWhitespace {} => Self::new("collapse_whitespace"),
            Step::Lowercase {} => Self::new("lowercase"),
        }
    }
}

/// What the buttons next to the filters do to the list of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StepOp {
    /// Add a filter of this kind at the end.
    Add(String),
    /// Remove the filter at this position.
    Remove(usize),
    /// Swap the filter at this position with the one before it.
    Up(usize),
    /// Swap the filter at this position with the one after it.
    Down(usize),
}

/// The whole form as typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Draft {
    /// Tells this form from any other open at the same time.
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) every: String,
    pub(crate) jitter: String,
    pub(crate) processor: String,
    pub(crate) steps: Vec<StepDraft>,
    pub(crate) trigger_text: String,
    pub(crate) block_text: String,
    /// Match the two lists against the whole new text, not only the added lines.
    pub(crate) scope_text: bool,
    pub(crate) notify_added: bool,
    pub(crate) notify_removed: bool,
    pub(crate) notify_changed: bool,
    pub(crate) timeout: String,
    pub(crate) user_agent: String,
    pub(crate) headers: String,
    pub(crate) retention: String,
    pub(crate) tags: Vec<String>,
    pub(crate) targets: Vec<String>,
    pub(crate) template: String,
}

impl Draft {
    /// An empty form for a new watch.
    pub(crate) fn blank(id: String) -> Self {
        Self {
            id,
            every: "1h".to_owned(),
            processor: "text".to_owned(),
            notify_added: true,
            notify_removed: true,
            notify_changed: true,
            retention: "100".to_owned(),
            ..Self::default()
        }
    }

    /// The form as it was posted.
    pub(crate) fn from_fields(fields: &Fields) -> Self {
        let mut at: Vec<usize> = fields
            .names()
            .filter_map(|name| name.strip_prefix("step-"))
            .filter_map(|rest| rest.split('-').next()?.parse().ok())
            .collect();
        at.sort_unstable();
        at.dedup();
        let text = |name: &str| fields.get(name).to_owned();
        Self {
            id: text("draft"),
            name: text("name"),
            url: text("url"),
            every: text("every"),
            jitter: text("jitter"),
            processor: text("processor"),
            steps: at
                .into_iter()
                .map(|at| StepDraft::from_fields(fields, at))
                .collect(),
            trigger_text: text("trigger_text"),
            block_text: text("block_text"),
            scope_text: fields.has("scope_text"),
            notify_added: fields.has("notify_added"),
            notify_removed: fields.has("notify_removed"),
            notify_changed: fields.has("notify_changed"),
            timeout: text("timeout"),
            user_agent: text("user_agent"),
            headers: text("headers"),
            retention: text("retention"),
            tags: fields.all("tag"),
            targets: fields.all("target"),
            template: text("template"),
        }
    }

    /// The form of a watch that exists.
    pub(crate) fn from_watch(
        id: String,
        watch: &Watch,
        tags: &[Tag],
        targets: &[NotificationTarget],
    ) -> Self {
        Self {
            id,
            name: watch.name.clone(),
            url: watch.url.clone(),
            every: crate::fmt::every(watch.schedule.interval().as_secs()),
            jitter: if watch.jitter_secs == 0 {
                String::new()
            } else {
                watch.jitter_secs.to_string()
            },
            processor: watch.processor.to_string(),
            steps: watch.pipeline.iter().map(StepDraft::from_step).collect(),
            trigger_text: watch.conditions.trigger_text.join("\n"),
            block_text: watch.conditions.block_text.join("\n"),
            scope_text: watch.conditions.scope == Scope::Text,
            notify_added: watch.conditions.notify_on.added,
            notify_removed: watch.conditions.notify_on.removed,
            notify_changed: watch.conditions.notify_on.changed,
            timeout: watch
                .fetch
                .timeout_secs
                .map(|s| s.to_string())
                .unwrap_or_default(),
            user_agent: watch.fetch.user_agent.clone().unwrap_or_default(),
            headers: watch
                .fetch
                .headers
                .iter()
                .map(|(name, value)| format!("{name}: {value}"))
                .collect::<Vec<_>>()
                .join("\n"),
            retention: watch.snapshot_retention.to_string(),
            tags: tags.iter().map(|tag| tag.id.to_string()).collect(),
            targets: targets.iter().map(|target| target.id.to_string()).collect(),
            template: watch.notification_template.clone().unwrap_or_default(),
        }
    }

    /// Change the list of filters as a button asked.
    pub(crate) fn apply(&mut self, op: &StepOp) {
        match op {
            StepOp::Add(kind) => {
                if STEP_KINDS.iter().any(|(known, _)| known == kind) {
                    self.steps.push(StepDraft::new(kind));
                }
            }
            StepOp::Remove(at) => {
                if *at < self.steps.len() {
                    self.steps.remove(*at);
                }
            }
            StepOp::Up(at) => {
                if *at > 0 && *at < self.steps.len() {
                    self.steps.swap(*at, *at - 1);
                }
            }
            StepOp::Down(at) => {
                if at + 1 < self.steps.len() {
                    self.steps.swap(*at, *at + 1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(kind: &str) -> StepDraft {
        StepDraft::new(kind)
    }

    fn draft_with(kinds: &[&str]) -> Draft {
        Draft {
            steps: kinds.iter().map(|kind| step(kind)).collect(),
            ..Draft::default()
        }
    }

    fn kinds(draft: &Draft) -> Vec<&str> {
        draft.steps.iter().map(|s| s.kind.as_str()).collect()
    }

    #[test]
    fn steps_are_added_removed_and_moved() {
        let mut draft = draft_with(&["css", "trim", "sort"]);
        draft.apply(&StepOp::Add("jq".into()));
        assert_eq!(kinds(&draft), ["css", "trim", "sort", "jq"]);
        draft.apply(&StepOp::Up(2));
        assert_eq!(kinds(&draft), ["css", "sort", "trim", "jq"]);
        draft.apply(&StepOp::Down(0));
        assert_eq!(kinds(&draft), ["sort", "css", "trim", "jq"]);
        draft.apply(&StepOp::Remove(1));
        assert_eq!(kinds(&draft), ["sort", "trim", "jq"]);
    }

    #[test]
    fn operations_out_of_range_or_of_an_unknown_kind_change_nothing() {
        let mut draft = draft_with(&["css", "trim"]);
        for op in [
            StepOp::Add("nonsense".into()),
            StepOp::Remove(5),
            StepOp::Up(0),
            StepOp::Up(9),
            StepOp::Down(1),
            StepOp::Down(9),
        ] {
            draft.apply(&op);
            assert_eq!(kinds(&draft), ["css", "trim"], "{op:?}");
        }
    }

    #[test]
    fn the_steps_of_a_posted_form_follow_their_numbers_not_their_order_in_the_body() {
        let fields = Fields::parse(
            "step-2-type=trim&step-0-type=css&step-0-selector=.price&step-0-required=on&step-10-type=sort",
        );
        let draft = Draft::from_fields(&fields);
        assert_eq!(kinds(&draft), ["css", "trim", "sort"]);
        assert_eq!(draft.steps[0].selector, ".price");
        assert!(draft.steps[0].required && !draft.steps[1].required);
    }

    #[test]
    fn a_form_without_checkboxes_means_they_were_unticked() {
        let draft = Draft::from_fields(&Fields::parse("url=https%3A%2F%2Fa.example"));
        assert!(!draft.notify_added && !draft.notify_removed && !draft.notify_changed);
        assert_eq!(draft.url, "https://a.example");
    }
}
