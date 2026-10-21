//! The request of a urlwatch job: its method, body, headers, cookies and timeout.

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use vigil_core::{FetchSettings, Method};

use super::is_set;
use crate::report::Note;

/// A scalar as the text a header or form field holds.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// The headers of a job, with its cookies joined into one `Cookie` header.
fn headers_of(job: &Map<String, Value>) -> BTreeMap<String, String> {
    let mut headers: BTreeMap<String, String> = job
        .get("headers")
        .and_then(Value::as_object)
        .map(|h| {
            h.iter()
                .filter_map(|(k, v)| Some((k.clone(), scalar(v)?)))
                .collect()
        })
        .unwrap_or_default();
    if let Some(cookies) = job.get("cookies").and_then(Value::as_object) {
        let joined: Vec<String> = cookies
            .iter()
            .filter_map(|(k, v)| Some(format!("{k}={}", scalar(v)?)))
            .collect();
        if !joined.is_empty() {
            headers.insert("Cookie".to_owned(), joined.join("; "));
        }
    }
    headers
}

/// What a job sends: the body, and the `Content-Type` that goes with it when the job names none.
fn body_of(job: &Map<String, Value>, data: &Value) -> Option<(String, &'static str)> {
    let json = job.get("json").and_then(Value::as_bool).unwrap_or(false);
    let kind = if json {
        "application/json"
    } else {
        "application/x-www-form-urlencoded"
    };
    match data {
        Value::String(text) => Some((text.clone(), if json { kind } else { "" })),
        Value::Object(map) if json => serde_json::to_string(map).ok().map(|b| (b, kind)),
        // urlwatch (requests) sends a mapping as a form.
        Value::Object(map) => Some((url_form(map), kind)),
        _ => None,
    }
}

/// The request settings of a job.
///
/// urlwatch sends `data` with POST when the job names no method. A job that names GET, or a
/// method that is not supported here, keeps GET and loses the body, which a GET does not send.
pub(super) fn request(job: &Map<String, Value>, notes: &mut Vec<Note>) -> FetchSettings {
    let mut fetch = FetchSettings::default();
    let mut headers = headers_of(job);

    let named = job.get("method").and_then(Value::as_str);
    if let Some(name) = named {
        match name.parse::<Method>() {
            Ok(method) => fetch.method = method,
            Err(_) => notes.push(Note::skipped(
                "method",
                "this request method is not supported; GET is used",
            )),
        }
    }
    if let Some(data) = job.get("data").filter(|d| is_set(d)) {
        match body_of(job, data) {
            None => notes.push(Note::skipped(
                "data",
                "data that is neither text nor a mapping has no equivalent and was left out",
            )),
            Some((body, content_type)) => {
                if fetch.method == Method::Get && named.is_none() {
                    fetch.method = Method::Post;
                    notes.push(Note::info(
                        "data",
                        "the job has data and no method, so it is requested with POST, as urlwatch does",
                    ));
                }
                if fetch.method == Method::Get {
                    notes.push(Note::skipped(
                        "data",
                        "a request body was left out because the method is GET",
                    ));
                } else {
                    if !content_type.is_empty()
                        && !headers
                            .keys()
                            .any(|k| k.eq_ignore_ascii_case("content-type"))
                    {
                        headers.insert("Content-Type".to_owned(), content_type.to_owned());
                    }
                    fetch.body = Some(body);
                }
            }
        }
    }
    for (name, value) in headers {
        if FetchSettings::header_is_valid(&name, &value) {
            fetch.headers.insert(name, value);
        } else {
            notes.push(Note::skipped(
                "headers",
                format!("the header {name:?} cannot be sent and was left out"),
            ));
        }
    }
    if let Some(timeout) = job.get("timeout").and_then(Value::as_u64)
        && FetchSettings::TIMEOUT_SECS.contains(&timeout)
    {
        fetch.timeout_secs = Some(timeout);
    }
    fetch
}

/// A mapping as an `application/x-www-form-urlencoded` body.
fn url_form(map: &Map<String, Value>) -> String {
    let mut out = String::new();
    for (key, value) in map {
        let Some(value) = scalar(value) else { continue };
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(&encode(key));
        out.push('=');
        out.push_str(&encode(&value));
    }
    out
}

fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte))
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
