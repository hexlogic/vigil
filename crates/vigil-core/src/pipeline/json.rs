//! Steps that work on a parsed JSON value.

use jsonpath::JsonPath;
use serde_json::Value;
use serde_json_path as jsonpath;

use super::error::PipelineErrorKind;
use crate::process::MAX_OUTPUT_BYTES;

/// A compiled JSON step.
#[derive(Debug)]
pub(super) enum JsonStep {
    Path(JsonPath),
}

pub(super) fn jsonpath(expr: &str) -> Result<JsonStep, PipelineErrorKind> {
    JsonPath::parse(expr)
        .map(JsonStep::Path)
        .map_err(|error| PipelineErrorKind::JsonPath {
            expr: expr.to_owned(),
            reason: error
                .to_string()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        })
}

impl JsonStep {
    pub(super) fn apply(&self, value: Value) -> Result<Value, PipelineErrorKind> {
        match self {
            Self::Path(path) => {
                let found = path.query(&value).all();
                if found.is_empty() {
                    return Err(PipelineErrorKind::NoMatch);
                }
                // Matches can overlap (`$..*` returns every value inside every other one), so
                // check the total before copying anything.
                let mut left = MAX_OUTPUT_BYTES;
                if !found.iter().all(|item| fits(item, &mut left)) {
                    return Err(PipelineErrorKind::TooLarge);
                }
                Ok(Value::Array(found.into_iter().cloned().collect()))
            }
        }
    }
}

/// Take the approximate size of `value` off `left`; false once it runs out.
fn fits(value: &Value, left: &mut usize) -> bool {
    match value {
        Value::String(text) => spend(left, text.len() + 2),
        Value::Array(items) => spend(left, 2) && items.iter().all(|item| fits(item, left)),
        Value::Object(map) => {
            let keys: usize = map.keys().map(|key| key.len() + 4).sum();
            spend(left, 2 + keys) && map.values().all(|item| fits(item, left))
        }
        _ => spend(left, 8),
    }
}

fn spend(left: &mut usize, cost: usize) -> bool {
    match left.checked_sub(cost) {
        Some(rest) => {
            *left = rest;
            true
        }
        None => false,
    }
}
