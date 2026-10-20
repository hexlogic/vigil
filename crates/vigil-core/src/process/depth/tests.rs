//! Tests of the nesting guard.

use super::nests_deeper_than;

fn nested(tag: &str, depth: usize) -> String {
    format!("<{tag}>").repeat(depth) + "x" + &format!("</{tag}>").repeat(depth)
}

#[test]
fn plain_nesting_is_counted() {
    assert!(nests_deeper_than(&nested("div", 513), 512));
    assert!(!nests_deeper_than(&nested("div", 512), 512));
    assert!(nests_deeper_than(&nested("DIV", 600), 512));
    assert!(
        nests_deeper_than(&"<b>".repeat(2_000), 512),
        "unclosed b nests"
    );
}

#[test]
fn a_huge_page_is_refused_without_reading_all_of_it() {
    let started = std::time::Instant::now();
    assert!(nests_deeper_than(&nested("div", 1_000_000), 512));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
fn stray_end_tags_do_not_hide_the_depth() {
    assert!(nests_deeper_than(&"<div></x>".repeat(2_000), 512));
    assert!(nests_deeper_than(
        &"<div></span></p></div2>".repeat(2_000),
        512
    ));
}

#[test]
fn balanced_siblings_do_not_add_up() {
    assert!(!nests_deeper_than(&"<div>x</div>".repeat(50_000), 512));
}

#[test]
fn elements_the_parser_closes_itself_are_not_counted() {
    assert!(!nests_deeper_than(&"<li>item".repeat(5_000), 512));
    assert!(!nests_deeper_than(&"<p>para".repeat(5_000), 512));
    assert!(!nests_deeper_than(&"<tr><td>a<td>b".repeat(5_000), 512));
    assert!(!nests_deeper_than(
        &"<br><img src=x><input>".repeat(5_000),
        512
    ));
}

#[test]
fn comments_declarations_and_raw_text_are_skipped() {
    let tags = "<div>".repeat(1_000);
    for wrapped in [
        format!("<!-- {tags} -->"),
        format!("<script>{tags}</script>"),
        format!("<style>{tags}</STYLE >"),
        format!("<textarea>{tags}</textarea>"),
        format!("<title>{tags}</title>"),
        format!("<script>{tags}"),
    ] {
        assert!(!nests_deeper_than(&wrapped, 512), "{}", &wrapped[..20]);
    }
    assert!(nests_deeper_than(&format!("<script>x</script>{tags}"), 512));
    // Like the tokenizer, a declaration or processing instruction ends at the first `>`.
    assert!(!nests_deeper_than(
        &"<!DOCTYPE html><?xml version=\"1.0\"?>".repeat(5_000),
        512
    ));
    assert!(nests_deeper_than(&format!("<!DOCTYPE html {tags}"), 512));
}

#[test]
fn quoted_attribute_values_may_contain_angle_brackets() {
    let tricky = r#"<div title="a > <div> b" data-x='<<'>"#.repeat(600);
    assert!(nests_deeper_than(&tricky, 512));
    let balanced = r#"<a href="x>y">text</a>"#.repeat(5_000);
    assert!(!nests_deeper_than(&balanced, 512));
}

#[test]
fn self_closing_svg_children_do_not_nest() {
    let paths = r#"<path d="M0 0"/>"#.repeat(5_000);
    assert!(!nests_deeper_than(&format!("<svg>{paths}</svg>"), 512));
    assert!(!nests_deeper_than(
        &format!("<svg><g>{paths}</g></svg>"),
        512
    ));
    assert!(!nests_deeper_than(
        &format!("<math>{}</math>", "<mspace/>".repeat(5_000)),
        512
    ));
    assert!(!nests_deeper_than(
        &format!("<svg>{}</svg>", "<g/>".repeat(5_000)),
        512
    ));
    assert!(!nests_deeper_than(&"<svg/>".repeat(5_000), 512));
    // A closing slash separated from the `>` is not self-closing.
    assert!(nests_deeper_than(
        &format!("<svg>{}", "<path / >".repeat(600)),
        512
    ));
}

#[test]
fn svg_children_that_are_not_closed_still_nest() {
    assert!(nests_deeper_than(
        &format!("<svg>{}", "<g>".repeat(600)),
        512
    ));
}

#[test]
fn html_elements_are_never_self_closing() {
    assert!(nests_deeper_than(&"<div/>".repeat(600), 512));
    assert!(nests_deeper_than(&"<span/>".repeat(600), 512));
    assert!(nests_deeper_than(&"<a href=x/>".repeat(600), 512));
}

#[test]
fn html_inside_foreign_content_follows_the_html_rules() {
    // `<div>` breaks out of svg, and foreignObject and title hold HTML again.
    assert!(nests_deeper_than(
        &format!("<svg>{}", "<div/>".repeat(600)),
        512
    ));
    assert!(nests_deeper_than(
        &format!("<svg><foreignObject>{}", "<section/>".repeat(600)),
        512
    ));
    assert!(nests_deeper_than(
        &format!("<svg><title>{}", "<div>".repeat(600)),
        512
    ));
}

#[test]
fn cdata_and_raw_text_names_in_svg_follow_foreign_rules() {
    let tags = "<div>".repeat(1_000);
    assert!(!nests_deeper_than(
        &format!("<svg><![CDATA[{tags}]]></svg>"),
        512
    ));
    // Outside svg, CDATA is a bogus comment that ends at the first `>`.
    assert!(nests_deeper_than(&format!("<![CDATA[{tags}]]>"), 512));
    // `<style>` inside svg is an ordinary element, so what follows is read as markup.
    assert!(nests_deeper_than(&format!("<svg><style>{tags}"), 512));
}

#[test]
fn every_way_to_end_a_comment_is_followed() {
    let tags = "<div>".repeat(1_000);
    for text in [
        format!("<!-->{tags}"),
        format!("<!--->{tags}"),
        format!("<!-- x --!>{tags}"),
        format!("<!-- x --->{tags}"),
        format!("<!-- x -->{tags}"),
    ] {
        assert!(nests_deeper_than(&text, 512), "{}", &text[..16]);
    }
    // A comment that is never closed hides everything after it.
    assert!(!nests_deeper_than(&format!("<!-- x {tags}"), 512));
    assert!(!nests_deeper_than(
        &format!("<!-- x -- y --- z {tags}"),
        512
    ));
}

#[test]
fn malformed_markup_does_not_panic() {
    for html in [
        "",
        "<",
        "<<",
        "</",
        "</>",
        "<!",
        "<!-",
        "<!--",
        "<?",
        "<a",
        "<a ",
        "<a b",
        "<a b=",
        "<a b='",
        "<a b=\"",
        "<a b=c",
        "<script",
        "<script>",
        "<script></",
        "<script></scrip",
        "<title>",
        "</div",
        "< div>",
        "<1>",
        "<a/>",
        "<a / >",
        "\u{0}<\u{0}>",
        "<svg",
        "<svg>",
        "<svg><![CDATA[",
        "</svg></svg>",
        "<svg><foreignObject></svg>",
        "<!-->",
        "<!--->",
        "<!---",
        "<!-- --",
        "<!-- --!",
        "<math><mi></math></mi>",
        "<svg><title></desc>",
    ] {
        let _ = nests_deeper_than(html, 512);
    }
}
