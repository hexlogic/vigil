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
fn css_include_that_finds_nothing_is_an_error() {
    let steps = r#"[{"type": "css", "selector": ".gone"}]"#;
    assert_eq!(
        error_of(Processor::Text, steps, NEWS),
        "step 1 (css): matched nothing"
    );
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
