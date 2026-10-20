//! Steps that select or remove parts of an HTML page.

use std::collections::HashSet;

use ego_tree::NodeId;
use scraper::{ElementRef, Html, Selector};

use super::error::PipelineErrorKind;
use super::step::SelectMode;
use crate::process::MAX_OUTPUT_BYTES;

/// A compiled HTML step.
#[derive(Debug)]
pub(super) enum HtmlStep {
    Css {
        selector: Selector,
        mode: SelectMode,
    },
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
                let ids: Vec<NodeId> = document.select(selector).map(|e| e.id()).collect();
                apply_selection(document, &ids, *mode)
            }
        }
    }
}

/// Keep or remove the elements with the given ids (in document order).
fn apply_selection(
    mut document: Html,
    ids: &[NodeId],
    mode: SelectMode,
) -> Result<Html, PipelineErrorKind> {
    match mode {
        SelectMode::Remove => {
            for id in ids {
                if let Some(mut node) = document.tree.get_mut(*id) {
                    node.detach();
                }
            }
            Ok(document)
        }
        SelectMode::Include => include(&document, ids),
    }
}

/// A new page holding only the selected elements, each on its own block.
///
/// An element inside another selected one is skipped, since its text is already part of the
/// outer one. Table rows and sections are wrapped in the table context an HTML parser needs
/// to keep them, table cells are reduced to their content, and every match goes in a `<div>`
/// so inline matches land on separate lines.
fn include(document: &Html, ids: &[NodeId]) -> Result<Html, PipelineErrorKind> {
    if ids.is_empty() {
        return Err(PipelineErrorKind::NoMatch);
    }
    let selected: HashSet<NodeId> = ids.iter().copied().collect();
    let mut body = String::new();
    for id in ids {
        let Some(node) = document.tree.get(*id) else {
            continue;
        };
        if node
            .ancestors()
            .any(|ancestor| selected.contains(&ancestor.id()))
        {
            continue;
        }
        let Some(element) = ElementRef::wrap(node) else {
            continue;
        };
        body.push_str("<div>");
        match element.value().name() {
            // A cell on its own has no table to sit in, and a table of one cell would be
            // drawn with borders, so keep what is inside it.
            "td" | "th" => body.push_str(&element.inner_html()),
            name => {
                let (open, close) = table_context(name);
                body.push_str(open);
                body.push_str(&element.html());
                body.push_str(close);
            }
        }
        body.push_str("</div>");
        if body.len() > MAX_OUTPUT_BYTES {
            return Err(PipelineErrorKind::TooLarge);
        }
    }
    Ok(Html::parse_document(&format!(
        "<html><body>{body}</body></html>"
    )))
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
