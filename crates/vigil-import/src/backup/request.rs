//! The request of a watch of a backup: method, body, headers, timeout, and what has no
//! equivalent in how the source fetched the page.

use serde_json::Value;
use vigil_core::{FetchSettings, Method};

use super::settings::{Globals, flag, text};
use crate::report::Note;

/// How the importer treats the timeout of a source whose default is not Vigil's.
const VIGIL_TIMEOUT_SECS: u64 = 30;

fn uses_browser(backend: &str) -> bool {
    backend.starts_with("html_webdriver") || backend.starts_with("extra_browser")
}

/// The request settings of `watch`, with the settings of the whole backup added.
pub(super) fn request(
    watch: &Value,
    globals: &Globals,
    backend: &str,
    notes: &mut Vec<Note>,
) -> FetchSettings {
    let mut fetch = FetchSettings::default();
    let method = text(watch.get("method")).unwrap_or("GET");
    match method.parse::<Method>() {
        Ok(method) => fetch.method = method,
        Err(_) => notes.push(Note::skipped(
            "method",
            "this request method is not supported; GET is used",
        )),
    }
    if let Some(body) = text(watch.get("body")) {
        if fetch.method == Method::Get {
            notes.push(Note::skipped(
                "body",
                "a request body was left out because the method is GET",
            ));
        } else {
            fetch.body = Some(body.to_owned());
        }
    }
    let mut headers = globals.headers.clone();
    if let Some(own) = watch.get("headers").and_then(Value::as_object) {
        for (name, value) in own {
            if let Some(value) = value.as_str() {
                headers.insert(name.clone(), value.to_owned());
            }
        }
    }
    for (name, value) in headers {
        if FetchSettings::header_is_valid(&name, &value) {
            fetch.headers.insert(name, value);
        } else {
            // Only the name: values often hold tokens.
            notes.push(Note::skipped(
                "headers",
                format!("the header {name:?} cannot be sent and was left out"),
            ));
        }
    }
    fetch.user_agent = globals.user_agent.clone().filter(|_| {
        !fetch
            .headers
            .keys()
            .any(|k| k.eq_ignore_ascii_case("user-agent"))
    });
    fetch.timeout_secs = globals
        .timeout
        .filter(|t| *t != VIGIL_TIMEOUT_SECS && FetchSettings::TIMEOUT_SECS.contains(t));
    if text(watch.get("proxy")).is_some() {
        notes.push(Note::skipped(
            "proxy",
            "a proxy for this watch has no equivalent yet",
        ));
    }
    if uses_browser(backend) {
        notes.push(Note::approximated("fetch_backend", "the watch used a browser; it is fetched over plain HTTP, so pages that need scripts will look different"));
    }
    if text(watch.get("webdriver_js_execute_code")).is_some() {
        notes.push(Note::skipped(
            "webdriver_js_execute_code",
            "running scripts in the page has no equivalent",
        ));
    }
    if flag(watch.get("ignore_status_codes"), false) {
        notes.push(Note::skipped(
            "ignore_status_codes",
            "pages that answer with an error status are still reported as errors",
        ));
    }
    fetch
}
