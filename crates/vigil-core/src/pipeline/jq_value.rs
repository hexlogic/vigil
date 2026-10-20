//! Converting between `serde_json` values and jaq values.

use jaq_json::{Num, Val};
use serde::Deserialize;
use serde_json::{Map, Number, Value};

/// Deepest nesting converted back to JSON, the same limit as the JSON printer.
const MAX_DEPTH: usize = 128;

/// Why a jaq value could not become a JSON value.
pub(super) enum Unconvertible {
    /// The value is bigger than the output limit.
    TooLarge,
    /// The value nests deeper than the printer accepts.
    TooDeep,
    /// An object key is not text.
    BadKey,
}

/// A JSON value as a jaq value.
pub(super) fn from_json(value: Value) -> Result<Val, serde_json::Error> {
    Val::deserialize(value)
}

/// A jaq value as a JSON value, taking its approximate size off `left` as it goes.
///
/// jaq shares sub-values, so a small program can describe an enormous tree (an array that
/// contains itself twice, ten levels over); `left` stops the copy instead of the memory.
/// What JSON cannot hold is mapped the way jq prints it: NaN becomes `null`, infinity the
/// largest finite number, and byte strings text with invalid bytes replaced.
pub(super) fn to_json(val: &Val, left: &mut usize) -> Result<Value, Unconvertible> {
    convert(val, left, 0)
}

fn convert(val: &Val, left: &mut usize, depth: usize) -> Result<Value, Unconvertible> {
    if depth > MAX_DEPTH {
        return Err(Unconvertible::TooDeep);
    }
    Ok(match val {
        Val::Null => spend(left, 8, Value::Null)?,
        Val::Bool(flag) => spend(left, 8, Value::Bool(*flag))?,
        Val::Num(number) => spend(left, 24, number_to_json(number))?,
        Val::BStr(bytes) | Val::TStr(bytes) => {
            let text = String::from_utf8_lossy(&bytes[..]).into_owned();
            spend(left, text.len() + 2, Value::String(text))?
        }
        Val::Arr(items) => {
            spend(left, 2, ())?;
            let mut converted = Vec::with_capacity(items.len().min(1024));
            for item in items.iter() {
                converted.push(convert(item, left, depth + 1)?);
            }
            Value::Array(converted)
        }
        Val::Obj(entries) => {
            spend(left, 2, ())?;
            let mut converted = Map::new();
            for (key, item) in entries.iter() {
                let key = match key {
                    Val::TStr(bytes) => String::from_utf8_lossy(&bytes[..]).into_owned(),
                    _ => return Err(Unconvertible::BadKey),
                };
                spend(left, key.len() + 4, ())?;
                converted.insert(key, convert(item, left, depth + 1)?);
            }
            Value::Object(converted)
        }
    })
}

/// Take `cost` off `left` and hand back `value`.
fn spend<T>(left: &mut usize, cost: usize, value: T) -> Result<T, Unconvertible> {
    *left = left.checked_sub(cost).ok_or(Unconvertible::TooLarge)?;
    Ok(value)
}

fn number_to_json(number: &Num) -> Value {
    match number {
        Num::Int(int) => i64::try_from(*int).map_or_else(|_| float(*int as f64), Value::from),
        Num::Float(float_value) => float(*float_value),
        other => {
            // Big integers and decimals: exact if they fit, else the nearest float.
            let text = other.to_string();
            text.parse::<i64>()
                .map(Value::from)
                .or_else(|_| text.parse::<u64>().map(Value::from))
                .unwrap_or_else(|_| float(text.parse().unwrap_or(f64::NAN)))
        }
    }
}

fn float(number: f64) -> Value {
    if number.is_nan() {
        Value::Null
    } else if number.is_infinite() {
        Number::from_f64(f64::MAX.copysign(number)).map_or(Value::Null, Value::Number)
    } else {
        Number::from_f64(number).map_or(Value::Null, Value::Number)
    }
}
