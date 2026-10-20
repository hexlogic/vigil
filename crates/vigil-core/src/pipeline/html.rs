//! Steps that select or remove parts of an HTML page.

use std::collections::HashSet;

use ego_tree::NodeId;
use scraper::{ElementRef, Html, Node, Selector};

use super::error::PipelineErrorKind;
use super::step::SelectMode;
use super::xpath::XPathStep;
use crate::process::MAX_OUTPUT_BYTES;

/// A compiled HTML step.
#[derive(Debug)]
pub(super) enum HtmlStep {
    Css {
        selector: Selector,
        mode: SelectMode,
    },
    XPath(XPathStep),
}

/// One thing a selection step found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Selected {
    /// An element or a text node of the page.
    Node(NodeId),
    /// Text that is not a node of the page, such as an attribute value or a computed result.
    Literal(String),
}

pub(super) fn css(selector: &str, mode: SelectMode) -> Result<HtmlStep, PipelineErrorKind> {
    let parsed = Selector::parse(selector).map_err(|error| PipelineErrorKind::Selector {
        selector: selector.to_owned(),
        reason: error
            .to_string()
            .lines()
            .next()
            .unwrap_or("invalid selector")
            .to_owned(),
    })?;
    Ok(HtmlStep::Css {
        selector: parsed,
        mode,
    })
}

/// Parse a page. Never fails: the HTML parser accepts any input.
pub(super) fn parse(text: &str) -> Html {
    Html::parse_document(text)
}

/// The page as HTML text, ready for the text conversion.
pub(super) fn serialize(document: &Html) -> String {
    document.html()
}

impl HtmlStep {
    pub(super) fn apply(&self, document: Html) -> Result<Html, PipelineErrorKind> {
        match self {
            Self::Css { selector, mode } => {
                let found: Vec<Selected> = document
                    .select(selector)
                    .map(|element| Selected::Node(element.id()))
                    .collect();
                apply_selection(document, &found, *mode)
            }
            Self::XPath(step) => {
                let found = step.select(&document)?;
                apply_selection(document, &found, step.mode())
            }
        }
    }
}

/// Keep or remove what a step found (in document order).
pub(super) fn apply_selection(
    mut document: Html,
    found: &[Selected],
    mode: SelectMode,
) -> Result<Html, PipelineErrorKind> {
    match mode {
        SelectMode::Remove => {
            for item in found {
                match item {
                    Selected::Node(id) => {
                        if let Some(mut node) = document.tree.get_mut(*id) {
                            node.detach();
                        }
                    }
                    Selected::Literal(_) => return Err(PipelineErrorKind::NotRemovable),
                }
            }
            Ok(document)
        }
        SelectMode::Include => include(&document, found),
    }
}

/// A new page holding only what was selected, each match in its own block.
///
/// A node inside another selected one is skipped, since its text is already part of the
/// outer one. Table rows and sections are wrapped in the table context an HTML parser needs
/// to keep them, table cells are reduced to their content, and every match goes in a
/// `<div>` so inline matches land on separate lines. Text and literal values are escaped.
fn include(document: &Html, found: &[Selected]) -> Result<Html, PipelineErrorKind> {
    let selected: HashSet<NodeId> = found
        .iter()
        .filter_map(|item| match item {
            Selected::Node(id) => Some(*id),
            Selected::Literal(_) => None,
        })
        .collect();
    let mut body = String::new();
    let mut kept = 0usize;
    for item in found {
        let before = body.len();
        match item {
            Selected::Literal(text) => push_text(&mut body, text),
            Selected::Node(id) => {
                let Some(node) = document.tree.get(*id) else {
                    continue;
                };
                if node
                    .ancestors()
                    .any(|ancestor| selected.contains(&ancestor.id()))
                {
                    continue;
                }
                match node.value() {
                    Node::Text(text) => push_text(&mut body, text),
                    Node::Element(_) => {
                        if let Some(element) = ElementRef::wrap(node) {
                            push_element(&mut body, &element);
                        }
                    }
                    _ => {}
                }
            }
        }
        kept += usize::from(body.len() > before);
        if body.len() > MAX_OUTPUT_BYTES {
            return Err(PipelineErrorKind::TooLarge);
        }
    }
    if kept == 0 {
        return Err(PipelineErrorKind::NoMatch);
    }
    Ok(Html::parse_document(&format!(
        "<html><body>{body}</body></html>"
    )))
}

fn push_text(body: &mut String, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    body.push_str("<div>");
    for ch in text.chars() {
        match ch {
            '&' => body.push_str("&amp;"),
            '<' => body.push_str("&lt;"),
            '>' => body.push_str("&gt;"),
            other => body.push(other),
        }
    }
    body.push_str("</div>");
}

fn push_element(body: &mut String, element: &ElementRef<'_>) {
    body.push_str("<div>");
    match element.value().name() {
        // A cell on its own has no table to sit in, and a table of one cell would be drawn
        // with borders, so keep what is inside it.
        "td" | "th" => body.push_str(&element.inner_html()),
        name => {
            let (open, close) = table_context(name);
            body.push_str(open);
            body.push_str(&element.html());
            body.push_str(close);
        }
    }
    body.push_str("</div>");
}

/// The markup an HTML parser needs around a table part to keep it.
fn table_context(name: &str) -> (&'static str, &'static str) {
    match name {
        "tr" => ("<table><tbody>", "</tbody></table>"),
        "thead" | "tbody" | "tfoot" | "caption" | "colgroup" => ("<table>", "</table>"),
        "col" => ("<table><colgroup>", "</colgroup></table>"),
        _ => ("", ""),
    }
}
