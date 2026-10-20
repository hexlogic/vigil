//! Snapshot tests of the XPath step.

use vigil_core::{Pipeline, Processor, Step};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const NEWS: &str = include_str!("fixtures/news.html");
const PRODUCT: &str = include_str!("fixtures/product.html");

fn render(steps: &str, body: &str) -> Result<String, Box<dyn std::error::Error>> {
    let steps: Vec<Step> = serde_json::from_str(steps)?;
    let pipeline = Pipeline::new(Processor::Text, &steps)?;
    Ok(pipeline.render(body.as_bytes(), Some("text/html"))?)
}

fn xpath(expr: &str, mode: &str, body: &str) -> Result<String, Box<dyn std::error::Error>> {
    let steps = serde_json::json!([{"type": "xpath", "expr": expr, "mode": mode}]);
    render(&steps.to_string(), body)
}

fn include(expr: &str, body: &str) -> Result<String, Box<dyn std::error::Error>> {
    xpath(expr, "include", body)
}

fn error_of(expr: &str, mode: &str, body: &str) -> String {
    xpath(expr, mode, body)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default()
}

#[test]
fn xpath_include_selects_elements() -> TestResult {
    assert_eq!(
        include("//h1[@class='title']", PRODUCT)?,
        "# Trail Backpack 30L\n"
    );
    insta::assert_snapshot!(include(
        "//div[@id='product-4821']//tr[th='Price']",
        PRODUCT
    )?);
    Ok(())
}

#[test]
fn xpath_include_matches_the_same_css_selection() -> TestResult {
    let by_css = render(r#"[{"type": "css", "selector": "article"}]"#, NEWS)?;
    assert_eq!(include("//article", NEWS)?, by_css);
    Ok(())
}

#[test]
fn xpath_remove_deletes_the_selected_elements() -> TestResult {
    let text = xpath("//nav | //footer | //p[@class='byline']", "remove", NEWS)?;
    insta::assert_snapshot!(text);
    let by_css = render(
        r#"[{"type": "css", "selector": "nav, footer, .byline", "mode": "remove"}]"#,
        NEWS,
    )?;
    assert_eq!(text, by_css);
    Ok(())
}

#[test]
fn xpath_include_selects_text_nodes() -> TestResult {
    insta::assert_snapshot!(include("//ul/li/text()", NEWS)?);
    Ok(())
}

#[test]
fn xpath_include_does_not_repeat_text_inside_a_selected_element() -> TestResult {
    assert_eq!(include("//li | //li/text()", NEWS)?, include("//li", NEWS)?);
    Ok(())
}

#[test]
fn xpath_include_selects_attribute_values() -> TestResult {
    assert_eq!(include("//nav/a/@href", NEWS)?, "/\n/world\n/local\n");
    Ok(())
}

#[test]
fn xpath_include_keeps_computed_values_as_text() -> TestResult {
    assert_eq!(include("count(//li)", NEWS)?, "3\n");
    assert_eq!(include("string(//h1)", NEWS)?, "Top stories\n");
    assert_eq!(include("boolean(//nav)", NEWS)?, "true\n");
    assert_eq!(include("count(//li) * 2", NEWS)?, "6\n");
    Ok(())
}

#[test]
fn xpath_text_is_escaped_not_parsed() -> TestResult {
    let html = "<p title='a &lt;b&gt; &amp; c'>x</p>";
    assert_eq!(include("//p/@title", html)?, "a <b> & c\n");
    Ok(())
}

#[test]
fn xpath_results_come_in_document_order() -> TestResult {
    assert_eq!(include("//h2 | //h1", NEWS)?, include("//h1 | //h2", NEWS)?);
    Ok(())
}

#[test]
fn xpath_include_that_finds_nothing_is_an_error() {
    for expr in [
        "//nothing",
        "string(//nothing)",
        "//p[@class='none']/text()",
    ] {
        assert_eq!(
            error_of(expr, "include", NEWS),
            "step 1 (xpath): matched nothing",
            "{expr}"
        );
    }
}

#[test]
fn xpath_remove_that_finds_nothing_leaves_the_page_alone() -> TestResult {
    assert_eq!(xpath("//nothing", "remove", NEWS)?, render("[]", NEWS)?);
    Ok(())
}

#[test]
fn xpath_remove_deletes_text_nodes() -> TestResult {
    let text = xpath("//p[@class='byline']/text()", "remove", NEWS)?;
    assert!(!text.contains("Mira Okafor"), "{text}");
    assert!(text.contains("Top stories"), "{text}");
    Ok(())
}

#[test]
fn xpath_remove_cannot_delete_values() {
    for expr in ["//a/@href", "count(//li)", "string(//h1)"] {
        assert_eq!(
            error_of(expr, "remove", NEWS),
            "step 1 (xpath): remove can only delete elements and text, not attribute values or computed results",
            "{expr}"
        );
    }
}

#[test]
fn xpath_selecting_the_root_keeps_the_page() -> TestResult {
    let whole = include("/", NEWS)?;
    assert!(
        whole.contains("Top stories") && whole.contains("Oats: unchanged"),
        "{whole}"
    );
    assert_eq!(xpath("/", "remove", NEWS)?, "");
    Ok(())
}

#[test]
fn xpath_chains_with_css() -> TestResult {
    let text = render(
        r#"[{"type": "css", "selector": ".product"},
            {"type": "xpath", "expr": "//td[@class='price' or @class='was-price']"}]"#,
        PRODUCT,
    )?;
    assert_eq!(text, "€89.90\n€109.00\n");
    Ok(())
}

#[test]
fn xpath_works_on_broken_html() -> TestResult {
    let html = "<div><p>one<p>two<b>bold</div><table><tr><td>cell";
    assert_eq!(include("//b", html)?, "bold\n");
    assert_eq!(include("count(//p)", html)?, "2\n");
    assert_eq!(include("//td", html)?, "cell\n");
    Ok(())
}

#[test]
fn xpath_copes_with_unusual_names() -> TestResult {
    let html = "<svg:rect xlink:href=a data-x=1 @click=2 1x=3></svg:rect><p ä=1>ok</p>";
    xpath("//*", "include", html)?;
    assert_eq!(include("//p", html)?, "ok\n");
    Ok(())
}

#[test]
fn xpath_refuses_pages_nested_too_deeply() -> TestResult {
    let nested = |depth: usize| format!("{}x{}", "<div>".repeat(depth), "</div>".repeat(depth));
    assert_eq!(include("//div", &nested(50))?, "x\n");
    let message = error_of("//div", "include", &nested(400));
    assert_eq!(
        message,
        "step 1 (xpath): invalid xpath \"//div\": the page nests elements deeper than 256 levels"
    );
    Ok(())
}

#[test]
fn an_invalid_expression_names_the_step() {
    for expr in ["//div[", "//", "div div", "@@"] {
        let message = error_of(expr, "include", "<p>x</p>");
        assert!(
            message.starts_with(&format!("step 1 (xpath): invalid xpath {expr:?}: ")),
            "{expr}: {message}"
        );
        assert!(!message.contains('\n'), "{expr}: {message}");
    }
}

#[test]
fn an_empty_expression_is_an_error() {
    for expr in ["", "  ", "child::"] {
        assert_eq!(
            error_of(expr, "include", "<p>x</p>"),
            format!(
                "step 1 (xpath): invalid xpath {expr:?}: the expression is empty or incomplete"
            )
        );
    }
}

#[test]
fn problems_are_explained_in_words() {
    for (expr, reason) in [
        ("//div[", "empty predicate"),
        ("//", "trailing slash"),
        ("div div", "right hand side expression missing"),
        ("frobnicate(//p)", "unknown function frobnicate"),
        ("$missing", "unknown variable missing"),
        ("//p[unknown:fn()]", "unknown namespace prefix unknown"),
    ] {
        assert_eq!(
            error_of(expr, "include", "<p>x</p>"),
            format!("step 1 (xpath): invalid xpath {expr:?}: {reason}")
        );
    }
}
