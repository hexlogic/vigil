//! The XPath step.
//!
//! XPath is evaluated by `sxd-xpath`, which works on its own document type. The page stays a
//! `scraper` tree (so CSS and XPath steps edit the same tree and html5ever parses everything);
//! for each XPath step a read-only copy of the tree is built, the expression is evaluated on
//! the copy, and the nodes it returns are mapped back to the page's node ids.

use std::collections::HashMap;

use ego_tree::{NodeId, iter::Edge};
use scraper::{Html, Node};
use sxd_document::{Package, dom};
use sxd_xpath::nodeset::Node as Found;
use sxd_xpath::{Context, Factory, Value, XPath};

use super::error::PipelineErrorKind;
use super::html::{HtmlStep, Selected};
use super::step::SelectMode;

/// Deepest element nesting that is copied. The evaluator walks the copy recursively, so a
/// page nested deeper than any real one could exhaust the stack.
const MAX_DEPTH: usize = 256;

/// A compiled XPath step.
#[derive(Debug)]
pub(super) struct XPathStep {
    expr: String,
    xpath: XPath,
    mode: SelectMode,
    required: bool,
}

pub(super) fn xpath(
    expr: &str,
    mode: SelectMode,
    required: bool,
) -> Result<HtmlStep, PipelineErrorKind> {
    let invalid = |reason: String| PipelineErrorKind::XPath {
        expr: expr.to_owned(),
        reason,
    };
    let empty = || invalid("the expression is empty or incomplete".to_owned());
    if expr.trim().is_empty() {
        return Err(empty());
    }
    let compiled = Factory::new()
        .build(expr)
        .map_err(|error| invalid(explain(&error.to_string())))?
        .ok_or_else(empty)?;
    Ok(HtmlStep::XPath(XPathStep {
        expr: expr.to_owned(),
        xpath: compiled,
        mode,
        required,
    }))
}

impl XPathStep {
    pub(super) fn mode(&self) -> SelectMode {
        self.mode
    }

    pub(super) fn required(&self) -> bool {
        self.required
    }

    /// Evaluate the expression on `document` and return what it found, in document order.
    ///
    /// Elements and text nodes come back as nodes of the page; attribute values and computed
    /// values (strings, numbers, booleans) come back as literals.
    pub(super) fn select(&self, document: &Html) -> Result<Vec<Selected>, PipelineErrorKind> {
        let package = Package::new();
        let copy = package.as_document();
        let mirror = Mirror::build(document, copy).map_err(|reason| PipelineErrorKind::XPath {
            expr: self.expr.clone(),
            reason,
        })?;
        let value = self
            .xpath
            .evaluate(&Context::new(), copy.root())
            .map_err(|error| PipelineErrorKind::XPath {
                expr: self.expr.clone(),
                reason: explain(&error.to_string()),
            })?;
        Ok(match value {
            Value::Nodeset(nodes) => nodes
                .document_order()
                .into_iter()
                .filter_map(|node| mirror.original(node))
                .collect(),
            // A computed empty string is "nothing", like an empty selection.
            Value::String(text) if text.is_empty() => Vec::new(),
            Value::String(text) => vec![Selected::Literal(text)],
            Value::Number(number) => vec![Selected::Literal(number.to_string())],
            Value::Boolean(flag) => vec![Selected::Literal(flag.to_string())],
        })
    }
}

/// A copy of a page's elements and text in `sxd-document`'s tree, with the way back.
struct Mirror<'d> {
    elements: HashMap<dom::Element<'d>, NodeId>,
    texts: HashMap<dom::Text<'d>, NodeId>,
    document_element: Option<NodeId>,
}

impl<'d> Mirror<'d> {
    fn build(document: &Html, copy: dom::Document<'d>) -> Result<Self, String> {
        let mut mirror = Self {
            elements: HashMap::new(),
            texts: HashMap::new(),
            document_element: None,
        };
        let root = copy.root();
        let mut open: Vec<dom::Element<'d>> = Vec::new();
        for edge in document.tree.root().traverse() {
            match edge {
                Edge::Open(node) => match node.value() {
                    Node::Element(element) => {
                        if open.len() >= MAX_DEPTH {
                            return Err(format!(
                                "the page nests elements deeper than {MAX_DEPTH} levels"
                            ));
                        }
                        let created = copy.create_element(element.name());
                        for (name, value) in element.attrs() {
                            created.set_attribute_value(name, value);
                        }
                        match open.last() {
                            Some(parent) => parent.append_child(created),
                            None => {
                                root.append_child(created);
                                mirror.document_element.get_or_insert(node.id());
                            }
                        }
                        mirror.elements.insert(created, node.id());
                        open.push(created);
                    }
                    Node::Text(text) => {
                        if let Some(parent) = open.last() {
                            let created = copy.create_text(text);
                            parent.append_child(created);
                            mirror.texts.insert(created, node.id());
                        }
                    }
                    _ => {}
                },
                Edge::Close(node) => {
                    if node.value().is_element() {
                        open.pop();
                    }
                }
            }
        }
        Ok(mirror)
    }

    /// What a node of the copy stands for in the page, if anything.
    fn original(&self, node: Found<'d>) -> Option<Selected> {
        match node {
            Found::Element(element) => self.elements.get(&element).copied().map(Selected::Node),
            Found::Text(text) => self.texts.get(&text).copied().map(Selected::Node),
            Found::Attribute(attribute) => Some(Selected::Literal(attribute.value().to_owned())),
            Found::Root(_) => self.document_element.map(Selected::Node),
            _ => None,
        }
    }
}

/// `sxd-xpath` reports problems as bare variant names (`EmptyPredicate`) or with `Debug`
/// output of its name type; turn both into plain words on one line.
fn explain(raw: &str) -> String {
    let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((what, rest)) = text.split_once(" OwnedPrefixedName {") {
        let name = rest
            .split("local_part: \"")
            .nth(1)
            .and_then(|tail| tail.split('"').next());
        if let Some(name) = name {
            return format!("{what} {name}");
        }
    }
    let is_variant_name = text.chars().all(char::is_alphanumeric)
        && text.chars().next().is_some_and(char::is_uppercase);
    if !is_variant_name {
        return text;
    }
    let mut words = String::new();
    for ch in text.chars() {
        if ch.is_uppercase() && !words.is_empty() {
            words.push(' ');
        }
        words.extend(ch.to_lowercase());
    }
    words
}
