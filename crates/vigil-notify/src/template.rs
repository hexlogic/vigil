//! Rendering message templates with per-channel escaping.
//!
//! Templates are written by the user, but the values they print come from arbitrary web pages.
//! Printing a value escapes it for the channel's markup; the template's own text is left alone.

use std::sync::{Arc, OnceLock};

use minijinja::value::{Enumerator, Object, Value};
use minijinja::{AutoEscape, Environment, Error, State, UndefinedBehavior, escape_formatter};

use crate::error::TemplateError;
use crate::escape::{Markup, code_block, escape, link};
use crate::event::Event;

/// How much work one render may do before it is stopped, so a loop in a template cannot stall
/// the thread that renders it.
const FUEL: u64 = 100_000;

/// A rendered message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rendered {
    /// The `title` the template set, or `<watch name> changed`.
    pub title: String,
    /// The message text.
    pub body: String,
}

/// The variables a template can use.
#[derive(Debug, Clone)]
pub(crate) struct Vars {
    pub watch_id: String,
    pub watch_name: String,
    pub watch_url: String,
    pub diff_added: String,
    pub diff_removed: String,
    pub diff_full: String,
    pub current_snapshot: String,
    pub triggered_text: Vec<String>,
    pub checked_at: String,
    pub vigil_base_url: String,
    pub diff_url: String,
    pub truncated: bool,
    pub is_test: bool,
}

impl Vars {
    pub(crate) fn from_event(event: &Event) -> Self {
        Self {
            watch_id: event.watch_id.clone(),
            watch_name: event.watch_name.clone(),
            watch_url: event.watch_url.clone(),
            diff_added: event.added.join("\n"),
            diff_removed: event.removed.join("\n"),
            diff_full: event.diff.clone(),
            current_snapshot: event.snapshot.clone(),
            triggered_text: event.triggered.clone(),
            checked_at: event.checked_at_rfc3339(),
            vigil_base_url: event.base_url.clone(),
            diff_url: event.diff_url.clone(),
            truncated: false,
            is_test: event.is_test,
        }
    }
}

impl Vars {
    /// The same variables without the long ones, which are what gets cut to fit a size limit.
    pub(crate) fn skeleton(&self) -> Self {
        Self {
            watch_id: self.watch_id.clone(),
            watch_name: self.watch_name.clone(),
            watch_url: self.watch_url.clone(),
            diff_added: String::new(),
            diff_removed: String::new(),
            diff_full: String::new(),
            current_snapshot: String::new(),
            triggered_text: Vec::new(),
            checked_at: self.checked_at.clone(),
            vigil_base_url: self.vigil_base_url.clone(),
            diff_url: self.diff_url.clone(),
            truncated: true,
            is_test: self.is_test,
        }
    }
}

impl Object for Vars {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let text = |s: &str| Some(Value::from(s));
        match key.as_str()? {
            "watch_id" => text(&self.watch_id),
            "watch_name" => text(&self.watch_name),
            "watch_url" => text(&self.watch_url),
            "diff_added" => text(&self.diff_added),
            "diff_removed" => text(&self.diff_removed),
            "diff_full" => text(&self.diff_full),
            "current_snapshot" => text(&self.current_snapshot),
            "triggered_text" => Some(Value::from(self.triggered_text.clone())),
            "checked_at" => text(&self.checked_at),
            "vigil_base_url" => text(&self.vigil_base_url),
            "diff_url" => text(&self.diff_url),
            "truncated" => Some(Value::from(self.truncated)),
            "is_test" => Some(Value::from(self.is_test)),
            _ => None,
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::NonEnumerable
    }
}

/// The markup the template being rendered is escaped for.
fn markup_of(state: &State) -> Markup {
    match state.auto_escape() {
        AutoEscape::Custom(name) => Markup::from_name(name).unwrap_or(Markup::Plain),
        _ => Markup::Plain,
    }
}

fn text_of(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

/// Print a value, escaping anything not marked safe for the channel's markup.
fn format_value(
    out: &mut minijinja::Output,
    state: &mut State,
    value: &Value,
) -> Result<(), Error> {
    match state.auto_escape() {
        AutoEscape::Custom(_) if !value.is_safe() => {
            let escaped = escape(markup_of(state), &text_of(value));
            out.write_str(&escaped).map_err(|_| {
                Error::new(minijinja::ErrorKind::WriteFailure, "could not write output")
            })
        }
        _ => escape_formatter(out, state, value),
    }
}

fn code_filter(state: &State, value: Value) -> Value {
    Value::from_safe_string(code_block(markup_of(state), &text_of(&value)))
}

fn link_filter(state: &State, value: Value) -> Value {
    Value::from_safe_string(link(markup_of(state), &text_of(&value)))
}

fn environment() -> &'static Environment<'static> {
    static ENV: OnceLock<Environment<'static>> = OnceLock::new();
    ENV.get_or_init(|| {
        let mut env = Environment::new();
        env.set_auto_escape_callback(|name| {
            match name.rsplit('.').next().and_then(Markup::from_name) {
                Some(Markup::Plain) | None => AutoEscape::None,
                Some(markup) => AutoEscape::Custom(markup.name().into()),
            }
        });
        env.set_undefined_behavior(UndefinedBehavior::Strict);
        env.set_fuel(Some(FUEL));
        env.set_formatter(format_value);
        // `|safe` would let a template print page text unescaped, which is what the escaping
        // exists to prevent.
        env.remove_filter("safe");
        env.add_filter("code", code_filter);
        env.add_filter("link", link_filter);
        env
    })
}

/// The title when the template sets none.
pub(crate) fn default_title(vars: &Vars) -> String {
    if vars.is_test {
        vars.watch_name.clone()
    } else {
        format!("{} changed", vars.watch_name)
    }
}

/// Render `source` for a channel with `markup`.
pub(crate) fn render(
    markup: Markup,
    source: &str,
    vars: &Arc<Vars>,
) -> Result<Rendered, TemplateError> {
    // The name tells the auto-escape callback which markup to escape for.
    let name = format!("template.{}", markup.name());
    let template = environment().template_from_named_str(&name, source)?;
    let rendered = template.render_captured(Value::from_dyn_object(vars.clone()))?;
    let title = rendered
        .state()
        .lookup("title")
        .filter(|title| !title.is_undefined() && !title.is_none())
        .map_or_else(|| default_title(vars), |title| text_of(&title));
    Ok(Rendered {
        title,
        body: rendered.output().trim().to_owned(),
    })
}

/// Check that a template compiles and renders, for every kind of channel, against a sample
/// event. Unknown variables and filters, and loops that run out of fuel, are errors.
pub fn validate_template(source: &str) -> Result<(), TemplateError> {
    let vars = Arc::new(Vars::from_event(&Event::sample()));
    for markup in [
        Markup::Plain,
        Markup::TelegramHtml,
        Markup::DiscordMarkdown,
        Markup::SlackMrkdwn,
    ] {
        render(markup, source, &vars)?;
    }
    Ok(())
}
