//! What every page needs: the language, the texts, links under the base path, rendering.

use std::convert::Infallible;
use std::sync::Arc;

use askama::Template;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};

use crate::AppState;
use crate::config::Auth;
use crate::i18n::{Lang, Translator};

/// Which entry of the menu is current.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Nav {
    /// None of them.
    None,
    /// The watch list and everything about one watch.
    Watches,
    /// Notification targets.
    Notifications,
    /// Tags.
    Tags,
    /// Settings.
    Settings,
}

/// The menu: the entry, where it leads and the key of its label.
const MENU: [(Nav, &str, &str); 4] = [
    (Nav::Watches, "/", "nav.watches"),
    (Nav::Notifications, "/notifications", "nav.notifications"),
    (Nav::Tags, "/tags", "nav.tags"),
    (Nav::Settings, "/settings", "nav.settings"),
];

/// One entry of the menu, ready to show.
pub(crate) struct MenuItem {
    pub(crate) url: String,
    pub(crate) label: String,
    pub(crate) current: bool,
}

/// What a template needs besides its own data. Handlers take it as an extractor and put it in
/// their template.
#[derive(Debug, Clone)]
pub(crate) struct Ctx {
    translator: Translator,
    base_path: Arc<str>,
    version: &'static str,
    no_auth: bool,
    nav: Nav,
}

impl Ctx {
    /// Mark `nav` as the current entry of the menu.
    pub(crate) fn nav(mut self, nav: Nav) -> Self {
        self.nav = nav;
        self
    }

    /// The translator, for code that formats text without a template.
    pub(crate) fn translator(&self) -> &Translator {
        &self.translator
    }

    /// The language code, for the `lang` attribute.
    pub(crate) fn lang_code(&self) -> &'static str {
        self.translator.lang().code()
    }

    /// The text for a key.
    pub(crate) fn t(&self, key: &str) -> String {
        self.translator.t(key)
    }

    /// The text for a key with one placeholder filled in.
    pub(crate) fn t1(&self, key: &str, name: &str, value: &str) -> String {
        self.translator.t1(key, name, value)
    }

    /// The text for a count in the right plural form.
    pub(crate) fn plural(&self, key: &str, n: usize) -> String {
        self.translator.plural(key, n)
    }

    /// The link to a path of this site (`/watch/1`), under the base path.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_path)
    }

    /// The link to an asset, with the version so a new release is not served from a cache.
    pub(crate) fn asset(&self, name: &str) -> String {
        format!("{}/assets/{name}?v={}", self.base_path, self.version)
    }

    /// The entries of the menu.
    pub(crate) fn menu(&self) -> Vec<MenuItem> {
        MENU.iter()
            .map(|&(nav, path, label)| MenuItem {
                url: self.url(path),
                label: self.t(label),
                current: nav == self.nav,
            })
            .collect()
    }

    /// The version of Vigil.
    pub(crate) fn version(&self) -> &'static str {
        self.version
    }

    /// Whether sign-in is off, which the layout warns about.
    pub(crate) fn no_auth(&self) -> bool {
        self.no_auth
    }

    /// Whether there is a session to end, so that the layout offers to sign out.
    pub(crate) fn auth_enabled(&self) -> bool {
        !self.no_auth
    }
}

impl Ctx {
    /// The context for a request with these headers.
    pub(crate) fn from_parts(state: &AppState, headers: &HeaderMap) -> Self {
        Ctx {
            translator: Translator::new(Arc::clone(&state.i18n), Lang::of_request(headers)),
            base_path: Arc::from(state.web.base_path()),
            version: state.web.version,
            no_auth: state.web.auth == Auth::Disabled,
            nav: Nav::None,
        }
    }
}

impl FromRequestParts<AppState> for Ctx {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Ctx::from_parts(state, &parts.headers))
    }
}

/// Render a template as an HTML page. A template that fails to render is a bug, not something
/// the visitor can fix: it is logged and answered with a plain 500.
pub(crate) fn render(template: &impl Template) -> Response {
    render_with(StatusCode::OK, template)
}

/// Like [`render`], with another status.
pub(crate) fn render_with(status: StatusCode, template: &impl Template) -> Response {
    match template.render() {
        Ok(html) => (status, Html(html)).into_response(),
        Err(error) => {
            tracing::error!(%error, "rendering a page failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}
