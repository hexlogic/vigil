//! The locale files: every language has the same texts, every text the UI asks for exists,
//! and every count has all the plural forms its language needs.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use vigil_web::{I18n, Lang};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn i18n() -> Result<I18n, vigil_web::LocaleError> {
    I18n::load()
}

fn keys(i18n: &I18n, lang: Lang) -> BTreeSet<String> {
    i18n.keys(lang).map(str::to_owned).collect()
}

/// A key with its plural form replaced by `#`, so that a count is one entry whichever forms a
/// language has for it: `watches.count.few` and `watches.count.other` are both `watches.count.#`.
fn without_form(key: &str) -> String {
    match key.rsplit_once('.') {
        Some((base, "one" | "few" | "many" | "other")) => format!("{base}.#"),
        _ => key.to_owned(),
    }
}

#[test]
fn every_language_has_the_same_keys() -> TestResult {
    let i18n = i18n()?;
    let entries = |lang| -> BTreeSet<String> {
        keys(&i18n, lang)
            .iter()
            .map(|key| without_form(key))
            .collect()
    };
    let (english, ukrainian) = (entries(Lang::En), entries(Lang::Uk));
    let only_en: Vec<_> = english.difference(&ukrainian).collect();
    let only_uk: Vec<_> = ukrainian.difference(&english).collect();
    assert!(only_en.is_empty(), "missing from uk.toml: {only_en:?}");
    assert!(only_uk.is_empty(), "missing from en.toml: {only_uk:?}");
    Ok(())
}

#[test]
fn placeholders_agree_between_languages() -> TestResult {
    let i18n = i18n()?;
    for key in i18n.keys(Lang::En) {
        let placeholders = |lang| {
            let text = i18n.get(lang, key).unwrap_or_default();
            let mut found = BTreeSet::new();
            for part in text.split('{').skip(1) {
                if let Some((name, _)) = part.split_once('}') {
                    found.insert(name.to_owned());
                }
            }
            found
        };
        // Plural forms may leave out {n}: "one" is sometimes written without the number.
        let (en, uk) = (placeholders(Lang::En), placeholders(Lang::Uk));
        let plural_form = key.ends_with(".one") || key.ends_with(".few") || key.ends_with(".many");
        if !plural_form {
            assert_eq!(en, uk, "placeholders of {key} differ");
        }
    }
    Ok(())
}

#[test]
fn every_count_has_the_forms_its_language_needs() -> TestResult {
    let i18n = i18n()?;
    for lang in Lang::ALL {
        let all = keys(&i18n, lang);
        let bases: BTreeSet<&str> = all
            .iter()
            .filter_map(|key| {
                let (base, form) = key.rsplit_once('.')?;
                ["one", "few", "many", "other"]
                    .contains(&form)
                    .then_some(base)
            })
            .collect();
        assert!(!bases.is_empty(), "the test would find no counts");
        for base in bases {
            for form in lang.plural_forms() {
                assert!(
                    all.contains(&format!("{base}.{form}")),
                    "{} needs {base}.{form}",
                    lang.code()
                );
            }
        }
    }
    Ok(())
}

#[test]
fn no_text_is_empty() -> TestResult {
    let i18n = i18n()?;
    for lang in Lang::ALL {
        for key in i18n.keys(lang) {
            let text = i18n.get(lang, key).unwrap_or_default();
            assert!(
                !text.trim().is_empty(),
                "{} has an empty {key}",
                lang.code()
            );
        }
    }
    Ok(())
}

/// The first part of every key: `nav`, `watches`, ...
fn namespaces(i18n: &I18n) -> BTreeSet<String> {
    i18n.keys(Lang::En)
        .filter_map(|key| key.split('.').next().map(str::to_owned))
        .collect()
}

fn files(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files(&path, extension, out);
        } else if path.extension().is_some_and(|e| e == extension) {
            out.push(path);
        }
    }
}

/// Every string literal in `source` that looks like a text key of one of the namespaces.
fn key_literals(source: &str, namespaces: &BTreeSet<String>) -> Vec<String> {
    let mut found = Vec::new();
    for quoted in source.split('"').skip(1).step_by(2) {
        let looks_like_key = quoted.contains('.')
            && !quoted.starts_with('.')
            && !quoted.ends_with('.')
            && quoted
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
        let is_file = ["html", "css", "js", "svg", "toml", "rs", "md"]
            .iter()
            .any(|ext| quoted.ends_with(&format!(".{ext}")));
        let namespace = quoted.split('.').next().unwrap_or_default();
        if looks_like_key && !is_file && namespaces.contains(namespace) {
            found.push(quoted.to_owned());
        }
    }
    found
}

#[test]
fn every_text_the_ui_asks_for_exists() -> TestResult {
    let i18n = i18n()?;
    let namespaces = namespaces(&i18n);
    let root = manifest_dir();
    let mut sources = Vec::new();
    files(&root.join("src"), "rs", &mut sources);
    files(&root.join("templates"), "html", &mut sources);
    assert!(
        sources.len() >= 5,
        "the test would scan nothing: {sources:?}"
    );

    let mut used: BTreeMap<String, PathBuf> = BTreeMap::new();
    for path in &sources {
        let source = fs::read_to_string(path).expect("source is readable");
        // The tests of a module are not UI.
        let source = source.split("#[cfg(test)]").next().unwrap_or_default();
        for key in key_literals(source, &namespaces) {
            used.entry(key).or_insert_with(|| path.clone());
        }
    }
    assert!(!used.is_empty(), "the test would find no keys");

    let known = keys(&i18n, Lang::En);
    for (key, path) in &used {
        // A count is asked for by its base name; its forms are checked separately.
        let exists = known.contains(key) || known.contains(&format!("{key}.other"));
        assert!(
            exists,
            "{} asks for the text {key:?}, which does not exist",
            path.display()
        );
    }
    Ok(())
}

#[test]
fn templates_hold_no_visible_text_of_their_own() {
    // A rough check: text between tags that is not an expression or the product name. Words
    // in templates belong in locales/*.toml.
    let root = manifest_dir().join("templates");
    let mut templates = Vec::new();
    files(&root, "html", &mut templates);
    for path in templates {
        let source = fs::read_to_string(&path).expect("template is readable");
        let mut text = String::new();
        let (mut in_tag, mut in_expr) = (false, false);
        let mut chars = source.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' if matches!(chars.peek(), Some('{' | '%' | '#')) => in_expr = true,
                '}' if in_expr && matches!(chars.peek(), Some('}')) => {
                    chars.next();
                    in_expr = false;
                }
                '%' | '#' if in_expr && matches!(chars.peek(), Some('}')) => {
                    chars.next();
                    in_expr = false;
                }
                '<' if !in_expr => in_tag = true,
                '>' if in_tag && !in_expr => in_tag = false,
                _ if !in_tag && !in_expr => text.push(c),
                _ => {}
            }
        }
        let words: Vec<&str> = text
            .split_whitespace()
            .filter(|word| word.chars().any(char::is_alphabetic))
            .collect();
        let allowed = ["Vigil", "·"];
        let stray: Vec<_> = words.iter().filter(|w| !allowed.contains(w)).collect();
        assert!(
            stray.is_empty(),
            "{} has text of its own: {stray:?}",
            path.display()
        );
    }
}
