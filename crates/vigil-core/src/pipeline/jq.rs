//! The jq step, run by `jaq`.
//!
//! jq programs are user input, so the engine is fenced in: programs are length- and
//! nesting-limited before they reach the parser, filters that read the environment, the
//! clock or other streams (or stop the process) are removed, and the number and size of
//! outputs are capped. A program that never produces output (`repeat(.)`, `def f: f; f`) is
//! not stopped here; the scheduler runs each check under a timeout.

use std::panic::{AssertUnwindSafe, catch_unwind};

use jaq_core::load::{self, Arena, File, Loader};
use jaq_core::{Compiler, Ctx, Vars, compile, data};
use jaq_json::Val;
use serde_json::Value;

use super::error::PipelineErrorKind;
use crate::process::MAX_OUTPUT_BYTES;

/// Longest program accepted, in bytes.
const MAX_PROGRAM_BYTES: usize = 16 * 1024;
/// Deepest nesting of brackets accepted. The parser and compiler recurse, so unbounded
/// nesting would overflow the stack, which cannot be recovered from.
const MAX_NESTING: usize = 64;
/// Most values a program may output.
pub(super) const MAX_OUTPUTS: usize = 10_000;

/// Native filters that are not available: they depend on the machine, the time or other
/// input, write to the console, or stop the program (`halt` would end the whole process).
const BLOCKED: &[&str] = &[
    "env",
    "now",
    "halt",
    "input",
    "inputs",
    "debug_empty",
    "stderr_empty",
    "input_filename",
    "localtime",
    "strflocaltime",
];

/// Definitions from the standard library that are built on the blocked filters. They are
/// left out too, since a definition that uses a missing filter fails to compile and would
/// take every program down with it.
const BLOCKED_DEFS: &[&str] = &["debug", "stderr", "halt", "halt_error"];

type Data = data::JustLut<Val>;

/// A checked jq program.
#[derive(Debug)]
pub(super) struct JqStep {
    program: String,
}

pub(super) fn jq(program: &str) -> Result<JqStep, PipelineErrorKind> {
    check_shape(program)?;
    compile_program(program)
        .map(|_| JqStep {
            program: program.to_owned(),
        })
        .map_err(PipelineErrorKind::JqProgram)
}

impl JqStep {
    /// Run the program on `value` and collect its outputs into an array.
    pub(super) fn apply(&self, value: Value) -> Result<Value, PipelineErrorKind> {
        let input = serde_json::to_vec(&value)
            .map_err(|error| PipelineErrorKind::JqRuntime(error.to_string()))?;
        let outputs = catch_unwind(AssertUnwindSafe(|| run(&self.program, &input)))
            .unwrap_or_else(|_| {
                Err(Failure::Message(
                    "the jq engine failed unexpectedly".to_owned(),
                ))
            })
            .map_err(|failure| match failure {
                Failure::Message(message) => PipelineErrorKind::JqRuntime(message),
                Failure::TooLarge => PipelineErrorKind::TooLarge,
            })?;
        if outputs.len() > MAX_OUTPUTS {
            return Err(PipelineErrorKind::JqTooManyOutputs(MAX_OUTPUTS));
        }
        Ok(Value::Array(outputs))
    }
}

/// Refuse programs that are too long or nested too deeply to parse safely.
fn check_shape(program: &str) -> Result<(), PipelineErrorKind> {
    if program.len() > MAX_PROGRAM_BYTES {
        return Err(PipelineErrorKind::JqProgram(format!(
            "the program is longer than {MAX_PROGRAM_BYTES} bytes"
        )));
    }
    let (mut depth, mut deepest) = (0usize, 0usize);
    for ch in program.chars() {
        match ch {
            '(' | '[' | '{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    if deepest > MAX_NESTING {
        return Err(PipelineErrorKind::JqProgram(format!(
            "brackets are nested deeper than {MAX_NESTING} levels"
        )));
    }
    Ok(())
}

/// Parse and compile `program`, returning a message on failure.
fn compile_program(program: &str) -> Result<(), String> {
    let arena = Arena::default();
    catch_unwind(AssertUnwindSafe(|| build(&arena, program).map(|_| ())))
        .unwrap_or_else(|_| Err("the jq engine failed unexpectedly".to_owned()))
}

type Built = jaq_core::Filter<Data>;

fn build<'s>(arena: &'s Arena, program: &'s str) -> Result<Built, String> {
    let defs = jaq_core::defs()
        .chain(jaq_std::defs().filter(|def| !BLOCKED_DEFS.contains(&def.name)))
        .chain(jaq_json::defs());
    let funs = jaq_core::funs::<Data>()
        .chain(jaq_std::funs::<Data>().filter(|fun| !BLOCKED.contains(&fun.0)))
        .chain(jaq_json::funs::<Data>());
    let modules = Loader::new(defs)
        .load(
            arena,
            File {
                code: program,
                path: (),
            },
        )
        .map_err(|errors| describe_load(&errors, program))?;
    Compiler::default()
        .with_funs(funs)
        .compile(modules)
        .map_err(|errors| describe_compile(&errors))
}

/// Why a run stopped.
enum Failure {
    Message(String),
    TooLarge,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

fn run(program: &str, input: &[u8]) -> Result<Vec<Value>, Failure> {
    let arena = Arena::default();
    let filter = build(&arena, program)?;
    let input = jaq_json::read::parse_single(input).map_err(|error| error.to_string())?;
    let context = Ctx::<Data>::new(&filter.lut, Vars::new([]));
    let mut outputs = Vec::new();
    let mut left = MAX_OUTPUT_BYTES;
    for result in filter.id.run((context, input)) {
        let value = match result {
            Ok(value) => value,
            // `halt` is not available, but never let one reach `process::exit`.
            Err(exn) => {
                return Err(Failure::Message(exn.get_err().map_or_else(
                    |_| "the program stopped early".to_owned(),
                    |error| error.to_string(),
                )));
            }
        };
        if outputs.len() >= MAX_OUTPUTS {
            // One more than the limit is enough for the caller to report it.
            outputs.push(Value::Null);
            break;
        }
        let text = value.to_string();
        left = left.checked_sub(text.len()).ok_or(Failure::TooLarge)?;
        outputs.push(serde_json::from_str(&text).map_err(|_| {
            Failure::Message("the program produced a value that is not JSON".to_owned())
        })?);
    }
    Ok(outputs)
}

/// Where in the program an error slice points, as a byte offset.
fn offset(program: &str, at: &str) -> Option<usize> {
    let start = at.as_ptr() as usize;
    let base = program.as_ptr() as usize;
    (start >= base && start <= base + program.len()).then(|| start - base)
}

fn describe_load(errors: &load::Errors<&str, ()>, program: &str) -> String {
    let at = |slice: &str| match offset(program, slice) {
        Some(position) => format!(" at byte {position}"),
        None => String::new(),
    };
    for (_file, error) in errors {
        match error {
            load::Error::Lex(list) => {
                if let Some((_expected, slice)) = list.first() {
                    return format!("syntax error{}", at(slice));
                }
            }
            load::Error::Parse(list) => {
                if let Some((_expected, slice)) = list.first() {
                    return format!("syntax error{}", at(slice));
                }
            }
            load::Error::Io(_) => return "modules and imports are not supported".to_owned(),
        }
    }
    "syntax error".to_owned()
}

fn describe_compile(errors: &compile::Errors<&str, ()>) -> String {
    for (_file, list) in errors {
        if let Some((name, undefined)) = list.first() {
            return match undefined {
                compile::Undefined::Filter(arity) => {
                    format!("filter {name}/{arity} is not defined")
                }
                other => format!("{} {name} is not defined", other.as_str()),
            };
        }
    }
    "the program could not be compiled".to_owned()
}
