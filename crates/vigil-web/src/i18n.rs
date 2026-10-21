//! UI text: the locale files, choosing a language and plural forms.
//!
//! Every string a person reads comes from `locales/<lang>.toml`. A key missing in a language
//! falls back to English; a key missing there shows as the key itself, which the tests in
//! `tests/locales.rs` make sure never ships.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::sync::Arc;

use axum::http::HeaderMap;
use axum::http::header::{ACCEPT_LANGUAGE, COOKIE};

/// Name of the cookie that holds the language a person picked.
pub const LANG_COOKIE: &str = "vigil_lang";

const EN: &str = include_str!("../../../locales/en.toml");
const UK: &str = include_str!("../../../locales/uk.toml");

/// A language the UI is available in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Lang {
    /// English, the language every missing text falls back to.
    En,
    /// Ukrainian.
    Uk,
}

impl Lang {
    /// Every language, English first.
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Uk];

    /// The language's code, as used in the `lang` attribute and the cookie.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Uk => "uk",
        }
    }

    /// The language's name in that language, for the language picker.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Uk => "Українська",
        }
    }

    /// The language for a code such as `uk` or `en-GB`, ignoring case and the region.
    pub fn from_code(code: &str) -> Option<Lang> {
        let primary = code.split(['-', '_']).next().unwrap_or_default();
        Lang::ALL
            .into_iter()
            .find(|lang| lang.code().eq_ignore_ascii_case(primary.trim()))
    }

    /// The language a request asks for: the cookie if it names one, else the best language in
    /// `Accept-Language`, else English.
    pub fn of_request(headers: &HeaderMap) -> Lang {
        cookie_lang(headers)
            .or_else(|| {
                headers
                    .get(ACCEPT_LANGUAGE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(Lang::negotiate)
            })
            .unwrap_or(Lang::En)
    }

    /// The best supported language in an `Accept-Language` header value, by quality and then
    /// by order. `*` and unknown languages are skipped.
    pub fn negotiate(header: &str) -> Option<Lang> {
        let mut best: Option<(u32, usize, Lang)> = None;
        for (position, item) in header.split(',').enumerate() {
            let mut parts = item.split(';');
            let Some(lang) = parts.next().and_then(Lang::from_code) else {
                continue;
            };
            let quality = parts
                .find_map(|param| param.trim().strip_prefix("q="))
                .map_or(1.0, |q| q.trim().parse::<f32>().unwrap_or(0.0));
            if quality <= 0.0 {
                continue;
            }
            // Thousandths keep the comparison exact; earlier entries win a tie.
            let score = (quality.clamp(0.0, 1.0) * 1000.0).round() as u32;
            if best.is_none_or(|(top, _, _)| score > top) {
                best = Some((score, position, lang));
            }
        }
        best.map(|(_, _, lang)| lang)
    }

    /// Which plural form of a text to use for `n`.
    fn plural_form(self, n: usize) -> &'static str {
        match self {
            Lang::En => {
                if n == 1 {
                    "one"
                } else {
                    "other"
                }
            }
            Lang::Uk => {
                let (ones, hundreds) = (n % 10, n % 100);
                if ones == 1 && hundreds != 11 {
                    "one"
                } else if (2..=4).contains(&ones) && !(12..=14).contains(&hundreds) {
                    "few"
                } else {
                    "many"
                }
            }
        }
    }

    /// The plural forms a language needs a text for.
    pub fn plural_forms(self) -> &'static [&'static str] {
        match self {
            Lang::En => &["one", "other"],
            Lang::Uk => &["one", "few", "many", "other"],
        }
    }
}

fn cookie_lang(headers: &HeaderMap) -> Option<Lang> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .filter_map(|cookie| cookie.trim().split_once('='))
        .find(|(name, _)| *name == LANG_COOKIE)
        .and_then(|(_, value)| Lang::from_code(value))
}

/// A locale file that could not be read.
#[derive(Debug, thiserror::Error)]
pub enum LocaleError {
    /// The file is not valid TOML.
    #[error("locale {lang}: {source}")]
    Syntax {
        /// The language.
        lang: &'static str,
        /// What the parser said.
        source: toml::de::Error,
    },
    /// A value is neither a string nor a table of them.
    #[error("locale {lang}: {key} is not text")]
    NotText {
        /// The language.
        lang: &'static str,
        /// The dotted key.
        key: String,
    },
}

/// The texts of every language.
#[derive(Debug)]
pub struct I18n {
    catalogs: BTreeMap<Lang, BTreeMap<String, String>>,
}

impl I18n {
    /// Read the locale files that are built into the binary.
    pub fn load() -> Result<Self, LocaleError> {
        let mut catalogs = BTreeMap::new();
        for (lang, source) in [(Lang::En, EN), (Lang::Uk, UK)] {
            catalogs.insert(lang, parse(lang.code(), source)?);
        }
        Ok(Self { catalogs })
    }

    /// The text for `key` in `lang`, without any fallback.
    pub fn get(&self, lang: Lang, key: &str) -> Option<&str> {
        self.catalogs
            .get(&lang)
            .and_then(|catalog| catalog.get(key))
            .map(String::as_str)
    }

    /// Every key of a language, in order.
    pub fn keys(&self, lang: Lang) -> impl Iterator<Item = &str> {
        self.catalogs
            .get(&lang)
            .into_iter()
            .flat_map(|catalog| catalog.keys().map(String::as_str))
    }
}

/// Flatten the tables of a locale file into dotted keys.
fn parse(lang: &'static str, source: &str) -> Result<BTreeMap<String, String>, LocaleError> {
    let table: toml::Table = source
        .parse()
        .map_err(|source| LocaleError::Syntax { lang, source })?;
    let mut out = BTreeMap::new();
    flatten(lang, "", &table, &mut out)?;
    Ok(out)
}

fn flatten(
    lang: &'static str,
    prefix: &str,
    table: &toml::Table,
    out: &mut BTreeMap<String, String>,
) -> Result<(), LocaleError> {
    for (name, value) in table {
        let key = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        match value {
            toml::Value::String(text) => {
                out.insert(key, text.clone());
            }
            toml::Value::Table(inner) => flatten(lang, &key, inner, out)?,
            _ => return Err(LocaleError::NotText { lang, key }),
        }
    }
    Ok(())
}

/// Looks texts up in one language. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Translator {
    lang: Lang,
    i18n: Arc<I18n>,
}

impl Translator {
    /// A translator for `lang`.
    pub fn new(i18n: Arc<I18n>, lang: Lang) -> Self {
        Self { lang, i18n }
    }

    /// The language texts come in.
    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// The text for `key`.
    pub fn t(&self, key: &str) -> String {
        self.i18n
            .get(self.lang, key)
            .or_else(|| self.i18n.get(Lang::En, key))
            .unwrap_or(key)
            .to_owned()
    }

    /// The text for `key` with `{name}` replaced by `value`.
    pub fn t1(&self, key: &str, name: &str, value: impl Display) -> String {
        fill(&self.t(key), &[(name, value.to_string())])
    }

    /// The text for `key` with two placeholders filled in.
    pub fn t2(
        &self,
        key: &str,
        first: &str,
        first_value: impl Display,
        second: &str,
        second_value: impl Display,
    ) -> String {
        fill(
            &self.t(key),
            &[
                (first, first_value.to_string()),
                (second, second_value.to_string()),
            ],
        )
    }

    /// The text for a count: `key.one`, `key.few`, `key.many` or `key.other`, whichever the
    /// language uses for `n`, with `{n}` replaced by it.
    pub fn plural(&self, key: &str, n: usize) -> String {
        let form = self.lang.plural_form(n);
        let text = [form, "other"]
            .into_iter()
            .find_map(|form| {
                let key = format!("{key}.{form}");
                self.i18n
                    .get(self.lang, &key)
                    .or_else(|| self.i18n.get(Lang::En, &key))
            })
            .unwrap_or(key);
        fill(text, &[("n", n.to_string())])
    }
}

fn fill(text: &str, values: &[(&str, String)]) -> String {
    let mut out = text.to_owned();
    for (name, value) in values {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn translator(lang: Lang) -> Translator {
        Translator::new(Arc::new(I18n::load().expect("locales parse")), lang)
    }

    #[test]
    fn codes_ignore_case_and_region() {
        assert_eq!(Lang::from_code("uk"), Some(Lang::Uk));
        assert_eq!(Lang::from_code("UK-ua"), Some(Lang::Uk));
        assert_eq!(Lang::from_code("en_GB"), Some(Lang::En));
        assert_eq!(Lang::from_code("de"), None);
        assert_eq!(Lang::from_code(""), None);
    }

    #[test]
    fn accept_language_picks_the_best_supported_language() {
        assert_eq!(Lang::negotiate("uk"), Some(Lang::Uk));
        assert_eq!(Lang::negotiate("de, uk;q=0.8, en;q=0.5"), Some(Lang::Uk));
        assert_eq!(Lang::negotiate("uk;q=0.4, en;q=0.9"), Some(Lang::En));
        assert_eq!(Lang::negotiate("en-GB,uk;q=0.9"), Some(Lang::En));
        assert_eq!(
            Lang::negotiate("uk, en"),
            Some(Lang::Uk),
            "the first wins a tie"
        );
        assert_eq!(Lang::negotiate("uk;q=0, en;q=0.1"), Some(Lang::En));
        assert_eq!(Lang::negotiate("*, de-DE"), None);
        assert_eq!(Lang::negotiate(""), None);
        assert_eq!(
            Lang::negotiate("uk;q=abc"),
            None,
            "an unreadable quality counts as 0"
        );
    }

    #[test]
    fn the_cookie_wins_over_the_header() {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("uk"));
        assert_eq!(Lang::of_request(&headers), Lang::Uk);
        headers.insert(COOKIE, HeaderValue::from_static("a=b; vigil_lang=en; c=d"));
        assert_eq!(Lang::of_request(&headers), Lang::En);
        headers.insert(COOKIE, HeaderValue::from_static("vigil_lang=xx"));
        assert_eq!(
            Lang::of_request(&headers),
            Lang::Uk,
            "an unknown cookie is ignored"
        );
        assert_eq!(Lang::of_request(&HeaderMap::new()), Lang::En);
    }

    #[test]
    fn english_plurals_have_one_and_other() {
        let en = translator(Lang::En);
        assert_eq!(en.plural("watches.count", 1), "1 watch");
        assert_eq!(en.plural("watches.count", 0), "0 watches");
        assert_eq!(en.plural("watches.count", 2), "2 watches");
    }

    #[test]
    fn ukrainian_plurals_follow_the_cldr_rules() {
        let forms: Vec<&str> = [
            0, 1, 2, 4, 5, 11, 12, 14, 19, 21, 22, 25, 101, 111, 112, 1000,
        ]
        .into_iter()
        .map(|n| Lang::Uk.plural_form(n))
        .collect();
        assert_eq!(
            forms,
            [
                "many", "one", "few", "few", "many", "many", "many", "many", "many", "one", "few",
                "many", "one", "many", "many", "many"
            ]
        );
        assert_eq!(
            translator(Lang::Uk).plural("watches.count", 5),
            "5 відстежень"
        );
    }

    #[test]
    fn a_missing_text_falls_back_to_english_then_to_the_key() {
        let i18n = Arc::new(I18n::load().expect("locales parse"));
        let uk = Translator::new(Arc::clone(&i18n), Lang::Uk);
        assert_eq!(uk.t("nav.watches"), "Відстеження");
        assert_eq!(uk.t("no.such.key"), "no.such.key");
        assert_eq!(uk.plural("no.such.key", 3), "no.such.key");
    }

    #[test]
    fn placeholders_are_filled_in() {
        let en = translator(Lang::En);
        assert_eq!(en.t1("layout.version", "version", "1.2"), "Vigil 1.2");
        assert_eq!(
            fill("{a} and {b}", &[("a", "1".into()), ("b", "2".into())]),
            "1 and 2"
        );
    }

    #[test]
    fn a_locale_value_that_is_not_text_is_refused() {
        assert!(matches!(
            parse("en", "[a]\nb = 3\n"),
            Err(LocaleError::NotText { .. })
        ));
        assert!(matches!(parse("en", "[a"), Err(LocaleError::Syntax { .. })));
    }
}
