//! Snapshot tests of the steps that select or remove parts of an HTML page.

use vigil_core::{Pipeline, Processor, Step};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const NEWS: &str = include_str!("fixtures/news.html");
const PRODUCT: &str = include_str!("fixtures/product.html");

fn pipeline(processor: Processor, steps: &str) -> Result<Pipeline, Box<dyn std::error::Error>> {
    let steps: Vec<Step> = serde_json::from_str(steps)?;
    Ok(Pipeline::new(processor, &steps)?)
}

fn render(steps: &str, body: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(pipeline(Processor::Text, steps)?.render(body.as_bytes(), Some("text/html"))?)
}

/// The message of the error from building or running the pipeline, or "" when it works.
fn error_of(processor: Processor, steps: &str, body: &str) -> String {
    let steps: Vec<Step> = match serde_json::from_str(steps) {
        Ok(steps) => steps,
        Err(error) => return error.to_string(),
    };
    Pipeline::new(processor, &steps)
        .map_err(|e| e.to_string())
        .and_then(|p| {
            p.render(body.as_bytes(), Some("text/html"))
                .map_err(|e| e.to_string())
        })
        .err()
        .unwrap_or_default()
}

#[test]
fn css_include_keeps_only_the_selected_elements() -> TestResult {
    insta::assert_snapshot!(render(r#"[{"type": "css", "selector": "article"}]"#, NEWS)?);
    Ok(())
}

#[test]
fn css_remove_drops_the_selected_elements() -> TestResult {
    let text = render(
        r#"[{"type": "css", "selector": "nav, footer, .byline", "mode": "remove"}]"#,
        NEWS,
    )?;
    insta::assert_snapshot!(text);
    Ok(())
}

#[test]
fn css_include_keeps_table_cells() -> TestResult {
    let text = render(r#"[{"type": "css", "selector": "td.price"}]"#, PRODUCT)?;
    assert_eq!(text, "€89.90\n");
    let text = render(r#"[{"type": "css", "selector": ".specs tr"}]"#, PRODUCT)?;
    insta::assert_snapshot!(text);
    Ok(())
}

#[test]
fn css_include_puts_inline_matches_on_separate_lines() -> TestResult {
    let text = render(r#"[{"type": "css", "selector": ".price"}]"#, PRODUCT)?;
    insta::assert_snapshot!(text);
    let badges = render(r#"[{"type": "css", "selector": "span.badge"}]"#, PRODUCT)?;
    assert_eq!(badges, "New\nFree shipping\n");
    Ok(())
}

#[test]
fn css_include_does_not_repeat_nested_matches() -> TestResult {
    let html = "<div><p>inner</p></div><p>outer</p>";
    let text = render(r#"[{"type": "css", "selector": "div, p"}]"#, html)?;
    assert_eq!(text, "inner\n\nouter\n");
    Ok(())
}

#[test]
fn css_include_that_finds_nothing_leaves_an_empty_page() -> TestResult {
    assert_eq!(
        render(r#"[{"type": "css", "selector": ".gone"}]"#, NEWS)?,
        ""
    );
    Ok(())
}

#[test]
fn css_required_turns_an_empty_selection_into_an_error() -> TestResult {
    for mode in ["include", "remove"] {
        let steps = format!(
            r#"[{{"type": "css", "selector": ".gone", "mode": "{mode}", "required": true}}]"#
        );
        assert_eq!(
            error_of(Processor::Text, &steps, NEWS),
            "step 1 (css): matched nothing",
            "{mode}"
        );
    }
    // A required selection that does find something is unaffected.
    let found = render(
        r#"[{"type": "css", "selector": "h1", "required": true}]"#,
        NEWS,
    )?;
    assert_eq!(found, "# Top stories\n");
    Ok(())
}

#[test]
fn css_required_is_satisfied_by_any_match() -> TestResult {
    let found = render(
        r#"[{"type": "css", "selector": "h1", "required": true}]"#,
        NEWS,
    )?;
    assert_eq!(found, "# Top stories\n");
    let steps = r#"[{"type": "css", "selector": "p", "required": true}]"#;
    assert_eq!(render(steps, "<p></p>")?, "");
    Ok(())
}

#[test]
fn css_remove_that_finds_nothing_leaves_the_page_alone() -> TestResult {
    let untouched = render(
        r#"[{"type": "css", "selector": "p", "mode": "remove"}]"#,
        "<i>x</i>",
    )?;
    assert_eq!(untouched, "x\n");
    let steps = r#"[{"type": "css", "selector": ".gone", "mode": "remove"}]"#;
    assert_eq!(render(steps, NEWS)?, render("[]", NEWS)?);
    Ok(())
}

#[test]
fn css_remove_of_the_whole_page_leaves_nothing() -> TestResult {
    for selector in ["html", "body", "*"] {
        let steps = format!(r#"[{{"type": "css", "selector": "{selector}", "mode": "remove"}}]"#);
        assert_eq!(render(&steps, NEWS)?, "", "{selector}");
    }
    Ok(())
}

#[test]
fn css_steps_chain() -> TestResult {
    let text = render(
        r#"[{"type": "css", "selector": ".product"},
            {"type": "css", "selector": ".badge, .description", "mode": "remove"},
            {"type": "ignore_lines", "patterns": ["SKU"]},
            {"type": "collapse_whitespace"}]"#,
        PRODUCT,
    )?;
    insta::assert_snapshot!(text);
    Ok(())
}

#[test]
fn css_forces_html_parsing_whatever_the_content_type() -> TestResult {
    let body = "<p class=a>kept</p><p>dropped</p>";
    let selected = pipeline(Processor::Text, r#"[{"type": "css", "selector": ".a"}]"#)?;
    for content_type in [Some("text/plain"), Some("application/json"), None] {
        assert_eq!(selected.render(body.as_bytes(), content_type)?, "kept\n");
    }
    Ok(())
}

#[test]
fn css_steps_see_the_decoded_page() -> TestResult {
    let (bytes, _, _) = encoding_rs::WINDOWS_1251
        .encode("<meta charset=windows-1251><p class=a>Привіт</p><p>інше</p>");
    let selected = pipeline(Processor::Text, r#"[{"type": "css", "selector": ".a"}]"#)?;
    assert_eq!(selected.render(&bytes, Some("text/html"))?, "Привіт\n");
    Ok(())
}

#[test]
fn css_steps_survive_odd_input() -> TestResult {
    let selected = pipeline(
        Processor::Text,
        r#"[{"type": "css", "selector": "p", "mode": "remove"}]"#,
    )?;
    for body in [
        "",
        "no markup at all",
        "<p",
        "<<<>>>",
        "<table><tr><td>x</table>",
        "</p></p>",
    ] {
        selected.render(body.as_bytes(), Some("text/html"))?;
    }
    Ok(())
}

#[test]
fn an_invalid_selector_names_the_step() {
    let message = error_of(
        Processor::Text,
        r#"[{"type": "css", "selector": "div["}]"#,
        "x",
    );
    assert!(
        message.starts_with("step 1 (css): invalid selector \"div[\": "),
        "{message}"
    );
    assert!(!message.contains('\n'), "{message}");
}

#[test]
fn html_steps_need_the_text_processor() {
    let steps = r#"[{"type": "css", "selector": "p"}]"#;
    for processor in [Processor::Json, Processor::Feed] {
        assert_eq!(
            error_of(processor, steps, "{}"),
            format!(
                "step 1 (css): html steps need the text processor, but the watch uses {processor}"
            )
        );
    }
}

#[test]
fn html_steps_must_come_before_text_steps() {
    let steps = r#"[{"type": "trim"}, {"type": "css", "selector": "p"}]"#;
    assert_eq!(
        error_of(Processor::Text, steps, "<p>x</p>"),
        "step 2 (css): html steps must come before text steps"
    );
}

#[test]
fn pages_nested_too_deeply_fail_before_any_step_runs() {
    let body = format!("{}x{}", "<div>".repeat(5_000), "</div>".repeat(5_000));
    for steps in [
        r#"[{"type": "css", "selector": "div"}]"#,
        r#"[{"type": "css", "selector": "div", "mode": "remove"}]"#,
        r#"[{"type": "xpath", "expr": "//div"}]"#,
    ] {
        assert_eq!(
            error_of(Processor::Text, steps, &body),
            "the page nests elements deeper than 512 levels",
            "{steps}"
        );
    }
}

#[test]
fn later_steps_do_not_see_what_an_earlier_remove_took_out() -> TestResult {
    let page = "<p>keep</p><div class=ad><p>AD</p><span>AD2</span></div><span>tail</span>";
    let remove_ad = r#"{"type": "css", "selector": ".ad", "mode": "remove"}"#;
    for next in [
        r#"{"type": "css", "selector": "p"}"#,
        r#"{"type": "css", "selector": "p, span, div"}"#,
        r#"{"type": "xpath", "expr": "//p | //span"}"#,
        r#"{"type": "css", "selector": "span", "mode": "remove"}"#,
    ] {
        let text = render(&format!("[{remove_ad}, {next}]"), page)?;
        assert!(!text.contains("AD"), "{next}: {text:?}");
        assert!(
            text.contains("keep") || next.contains("remove"),
            "{next}: {text:?}"
        );
    }
    let xpath_first = r#"{"type": "xpath", "expr": "//div", "mode": "remove"}"#;
    let text = render(
        &format!(r#"[{xpath_first}, {{"type": "css", "selector": "p"}}]"#),
        page,
    )?;
    assert_eq!(text, "keep\n");
    Ok(())
}

#[test]
fn removing_everything_leaves_nothing_for_later_steps() -> TestResult {
    for first in ["*", "html", "body"] {
        let steps = format!(
            r#"[{{"type": "css", "selector": "{first}", "mode": "remove"}},
                {{"type": "css", "selector": "p"}},
                {{"type": "xpath", "expr": "//p"}}]"#
        );
        assert_eq!(render(&steps, "<p>x</p>")?, "", "{first}");
    }
    Ok(())
}

#[test]
fn css_matches_come_in_document_order() -> TestResult {
    // A table cell outside a row is moved in front of the table by the parser; the matches
    // must still be listed as they are in the tree.
    let text = render(
        r#"[{"type": "css", "selector": "p, td"}]"#,
        "<p>one</p><table><p>two</p><tr><td>three</td></tr></table>",
    )?;
    assert_eq!(
        text.lines().filter(|l| !l.is_empty()).count(),
        3,
        "{text:?}"
    );
    Ok(())
}

#[test]
fn selectors_that_are_too_long_or_too_nested_are_refused() {
    // Parsing them recurses, and a stack overflow cannot be caught.
    for selector in [
        format!("{}p", "div > ".repeat(200)),
        format!("{}p{}", ":not(".repeat(100), ")".repeat(100)),
        format!("{}p{}", ":not(".repeat(10_000), ")".repeat(10_000)),
    ] {
        let steps = serde_json::json!([{"type": "css", "selector": selector}]).to_string();
        let message = error_of(Processor::Text, &steps, "<p>x</p>");
        assert!(
            message.starts_with("step 1 (css): invalid selector \"")
                && (message.ends_with("is longer than 1024 bytes")
                    || message.ends_with("brackets are nested deeper than 32 levels")),
            "{message}"
        );
        assert!(
            message.len() < 200,
            "the message repeats the whole selector"
        );
    }
    let fine = format!("{}p{}", ":not(".repeat(10), ")".repeat(10));
    let steps = serde_json::json!([{"type": "css", "selector": fine}]).to_string();
    assert_eq!(error_of(Processor::Text, &steps, "<p>x</p>"), "");
}
