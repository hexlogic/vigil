//! An OPML outline of feeds, as feed readers export it.
//!
//! An `<outline>` with an `xmlUrl` is a feed and becomes a watch with the `feed` processor; one
//! with only a `url` or `htmlUrl` becomes a page watch. The outline it sits in, if it has a
//! title, becomes its tag.

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use vigil_core::{Processor, Schedule};
use vigil_fetch::check_url;
use vigil_store::{ImportTag, ImportWatch, NewWatch};

use crate::builder::{Builder, Existing};
use crate::report::Note;
use crate::text::short_url;
use crate::urls::TAG_NAME_LEN;
use crate::{Format, ImportError, ImportPlan, Options};

/// Deepest nesting of outlines that is followed.
const MAX_DEPTH: usize = 64;

/// What an outline says.
struct Outline {
    title: String,
    xml_url: Option<String>,
    page_url: Option<String>,
}

fn outline_of(start: &BytesStart<'_>) -> Outline {
    let (mut text, mut title, mut xml_url, mut html_url, mut url) = (None, None, None, None, None);
    for attribute in start.attributes().flatten() {
        let Ok(value) = attribute.normalized_value(XmlVersion::Implicit1_0) else {
            continue;
        };
        let value = value.trim().to_owned();
        match attribute.key.local_name().as_ref() {
            "text" => text = Some(value),
            "title" => title = Some(value),
            "xmlUrl" => xml_url = Some(value),
            "htmlUrl" => html_url = Some(value),
            "url" => url = Some(value),
            _ => {}
        }
    }
    Outline {
        title: title.or(text).unwrap_or_default(),
        xml_url: xml_url.filter(|v| !v.is_empty()),
        page_url: url.or(html_url).filter(|v| !v.is_empty()),
    }
}

pub(crate) fn plan(
    bytes: &[u8],
    options: &Options,
    existing: &Existing,
) -> Result<ImportPlan, ImportError> {
    let invalid = |reason: &str| ImportError::Invalid {
        format: Format::Opml,
        reason: reason.to_owned(),
    };
    let text = std::str::from_utf8(bytes)
        .map_err(|_| invalid("the file is not UTF-8 text"))?
        .trim_start_matches('\u{feff}');
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut builder = Builder::new(Format::Opml, options, existing);
    let every = options.every.as_secs();
    // The titles of the folders we are inside, innermost last; `None` for an outline that
    // is a feed, which holds no folder.
    let mut folders: Vec<Option<String>> = Vec::new();
    let mut seen_opml = false;
    loop {
        let event = reader
            .read_event()
            .map_err(|error| invalid(&format!("the XML is not well formed: {error}")))?;
        let (start, closes) = match &event {
            Event::Start(start) => (Some(start), false),
            Event::Empty(start) => (Some(start), true),
            Event::End(end) => {
                if end.local_name().as_ref() == "outline" {
                    folders.pop();
                }
                (None, false)
            }
            Event::Eof => break,
            _ => (None, false),
        };
        let Some(start) = start else { continue };
        match start.local_name().as_ref() {
            "opml" => seen_opml = true,
            "outline" => {
                let outline = outline_of(start);
                let folder = (outline.xml_url.is_none()
                    && outline.page_url.is_none()
                    && !outline.title.is_empty())
                .then(|| outline.title.clone());
                if let Some(url) = outline.xml_url.clone().or(outline.page_url.clone()) {
                    builder.room()?;
                    let is_feed = outline.xml_url.is_some();
                    let source = if outline.title.is_empty() {
                        short_url(&url)
                    } else {
                        outline.title.clone()
                    };
                    if let Err(error) = check_url(&url) {
                        let why = match error {
                            vigil_fetch::FetchError::UnsupportedScheme(_) => {
                                "only http and https addresses can be watched"
                            }
                            _ => "this is not an address",
                        };
                        builder.skip(source, vec![Note::skipped("url", why)]);
                    } else {
                        let mut notes = Vec::new();
                        let mut tags = Vec::new();
                        if let Some(Some(name)) = folders.iter().rev().find(|f| f.is_some()) {
                            if name.chars().count() > TAG_NAME_LEN {
                                notes.push(Note::skipped(
                                    "tags",
                                    format!(
                                        "a folder name over {TAG_NAME_LEN} characters was left out"
                                    ),
                                ));
                            } else {
                                builder.define_tag(ImportTag {
                                    name: name.clone(),
                                    default_interval_secs: None,
                                    target_urls: Vec::new(),
                                });
                                tags.push(name.clone());
                            }
                        }
                        let mut settings = NewWatch::new(&url, Schedule::Interval(every));
                        if !outline.title.is_empty() {
                            settings.name = outline.title.clone();
                        }
                        if is_feed {
                            settings.processor = Processor::Feed;
                        }
                        builder.add(
                            source,
                            ImportWatch {
                                settings,
                                paused: false,
                                tags,
                                target_urls: Vec::new(),
                                template: None,
                            },
                            notes,
                        );
                    }
                }
                if !closes {
                    if folders.len() >= MAX_DEPTH {
                        return Err(invalid("the outlines are nested too deeply"));
                    }
                    folders.push(folder);
                }
            }
            _ => {}
        }
    }
    if !seen_opml {
        return Err(invalid("there is no <opml> element"));
    }
    Ok(builder.finish())
}
