//! Runs pipelines over arbitrary bytes. Errors are fine; panics, hangs and stack overflows
//! are the findings.
//!
//! jq programs are not fuzzed: a generated program can legitimately run forever, which a
//! fuzzer can only report as a timeout.

#![no_main]

use libfuzzer_sys::fuzz_target;
use vigil_core::{Pipeline, Processor, Step};

/// The pipelines every input is run through, as (processor, stored steps).
const PIPELINES: &[(Processor, &str)] = &[
    (Processor::Text, "[]"),
    (
        Processor::Text,
        r##"[{"type": "css", "selector": "article, p, td, li, .price, #main"}]"##,
    ),
    (
        Processor::Text,
        r##"[{"type": "css", "selector": "nav, footer, script, style, .ad, *", "mode": "remove"}]"##,
    ),
    (
        Processor::Text,
        r##"[{"type": "xpath", "expr": "//p | //td | //tr | //a/@href | //text()"}]"##,
    ),
    (
        Processor::Text,
        r##"[{"type": "xpath", "expr": "//div[@class] | //table | //text()", "mode": "remove"}]"##,
    ),
    (
        Processor::Text,
        r##"[{"type": "xpath", "expr": "count(//*) + string-length(string(/))"}]"##,
    ),
    (
        Processor::Text,
        r##"[{"type": "css", "selector": "body, main, div"},
            {"type": "xpath", "expr": "//script", "mode": "remove"},
            {"type": "regex_replace", "pattern": "\\d+", "replacement": "#"},
            {"type": "regex_extract", "pattern": "(\\w+)\\s", "group": 1},
            {"type": "ignore_lines", "patterns": ["/x/i", "ad"]},
            {"type": "sort"}, {"type": "unique"}, {"type": "collapse_whitespace"},
            {"type": "trim"}, {"type": "lowercase"}]"##,
    ),
    (
        Processor::Json,
        r##"[{"type": "jsonpath", "expr": "$..*"}, {"type": "lowercase"}]"##,
    ),
    (Processor::Json, "[]"),
    (Processor::Feed, "[]"),
];

/// Content types worth trying for each processor: the right one, and none (which makes the
/// text processor sniff).
fn content_types(processor: Processor) -> &'static [Option<&'static str>] {
    match processor {
        Processor::Text => &[Some("text/html; charset=utf-8"), None],
        Processor::Json => &[Some("application/json")],
        Processor::Feed => &[Some("application/rss+xml")],
    }
}

thread_local! {
    static COMPILED: Vec<Pipeline> = PIPELINES
        .iter()
        .map(|(processor, steps)| {
            let steps: Vec<Step> = serde_json::from_str(steps).expect("fuzz pipelines are valid");
            Pipeline::new(*processor, &steps).expect("fuzz pipelines compile")
        })
        .collect();
}

fuzz_target!(|data: &[u8]| {
    COMPILED.with(|pipelines| {
        for (pipeline, (processor, _)) in pipelines.iter().zip(PIPELINES) {
            for content_type in content_types(*processor) {
                let _ = pipeline.render(data, *content_type);
            }
        }
    });
});
