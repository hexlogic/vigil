//! Settings: the language, the password, sessions, and a few facts about this server.

use std::time::Instant;

use askama::Template;
use axum::extract::{Extension, Query, State};
use axum::http::StatusCode;
use axum::http::header::SET_COOKIE;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use super::api_tokens::TokensView;
use super::server_error;
use crate::AppState;
use crate::auth::session::{clear_cookie, language_cookie};
use crate::auth::{SessionKey, change_password, login_url, verify_admin_password};
use crate::config::Auth;
use crate::form::Fields;
use crate::i18n::Lang;
use crate::page::{Ctx, Nav, render, render_with};

/// Shortest password the form accepts.
const MIN_PASSWORD_LEN: usize = 8;

/// A language the picker offers.
struct LanguageOption {
    code: &'static str,
    name: &'static str,
    selected: bool,
}

#[derive(Template)]
#[template(path = "settings.html")]
pub(crate) struct Settings {
    ctx: Ctx,
    /// What was just done, if anything: a key of a sentence.
    done: Option<&'static str>,
    languages: Vec<LanguageOption>,
    /// Whether there is a password to change, and a session to end.
    sign_in: bool,
    /// The password is set in the environment and cannot be changed here.
    password_from_env: bool,
    password_error: Option<String>,
    base_url: Option<String>,
    base_url_missing: bool,
    version: &'static str,
    language_url: String,
    password_url: String,
    sign_out_url: String,
    tokens: TokensView,
}

/// Query of the settings page.
#[derive(Debug, Deserialize)]
pub(crate) struct SettingsQuery {
    #[serde(default)]
    done: String,
}

/// The page with these API tokens on it.
pub(crate) fn page_with(
    state: &AppState,
    ctx: Ctx,
    done: Option<&'static str>,
    password_error: Option<String>,
    tokens: TokensView,
) -> Settings {
    let current = ctx.translator().lang();
    Settings {
        done,
        languages: Lang::ALL
            .into_iter()
            .map(|lang| LanguageOption {
                code: lang.code(),
                name: lang.native_name(),
                selected: lang == current,
            })
            .collect(),
        sign_in: state.web.auth == Auth::Password,
        password_from_env: state.web.password_from_env,
        password_error,
        base_url: state.web.base_url.clone(),
        base_url_missing: state.web.base_url.is_none(),
        version: state.web.version,
        language_url: ctx.url("/settings/language"),
        password_url: ctx.url("/settings/password"),
        sign_out_url: ctx.url("/settings/sign-out-everywhere"),
        tokens,
        ctx: ctx.nav(Nav::Settings),
    }
}

/// `GET /settings`.
pub(crate) async fn settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Query(query): Query<SettingsQuery>,
) -> Response {
    // Only the sentences we know can be asked for, so that an address cannot write the page.
    let done = match query.done.as_str() {
        "language" => Some("settings.done.language"),
        "password" => Some("settings.done.password"),
        "token_revoked" => Some("settings.done.token_revoked"),
        _ => None,
    };
    let tokens = match TokensView::load(&state, &ctx).await {
        Ok(tokens) => tokens,
        Err(error) => return server_error(ctx, &error),
    };
    render(&page_with(&state, ctx, done, None, tokens))
}

/// `POST /settings/language`.
pub(crate) async fn set_language(
    State(state): State<AppState>,
    ctx: Ctx,
    body: String,
) -> Response {
    let fields = Fields::parse(&body);
    let Some(lang) = Lang::from_code(fields.get("lang")) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let mut response = Redirect::to(&ctx.url("/settings?done=language")).into_response();
    if let Some(cookie) = language_cookie(&state.web, lang.code()) {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    response
}

/// `POST /settings/password`.
pub(crate) async fn set_password(
    State(state): State<AppState>,
    ctx: Ctx,
    key: Option<Extension<SessionKey>>,
    body: String,
) -> Response {
    // With sign-in off there is no password, and with one from the environment a change here
    // would be undone at the next start.
    if state.web.auth != Auth::Password || state.web.password_from_env {
        return StatusCode::NOT_FOUND.into_response();
    }
    let t = ctx.translator().clone();
    let tokens = match TokensView::load(&state, &ctx).await {
        Ok(tokens) => tokens,
        Err(error) => return server_error(ctx, &error),
    };
    let fields = Fields::parse(&body);
    let (current, new, again) = (
        fields.get("current"),
        fields.get("new"),
        fields.get("again"),
    );

    let refuse = |status: StatusCode, key: &str, ctx: Ctx, tokens: TokensView| {
        render_with(
            status,
            &page_with(&state, ctx, None, Some(t.t(key)), tokens),
        )
    };
    if let Some(wait) = state.limiter.wait(Instant::now()) {
        let mut response = refuse(StatusCode::TOO_MANY_REQUESTS, "login.too_many", ctx, tokens);
        if let Ok(seconds) = wait.as_secs().max(1).to_string().parse() {
            response
                .headers_mut()
                .insert(axum::http::header::RETRY_AFTER, seconds);
        }
        return response;
    }
    match verify_admin_password(&state.store, current).await {
        Ok(true) => {}
        Ok(false) => {
            state.limiter.record_failure(Instant::now());
            tokio::time::sleep(state.web.failed_login_delay).await;
            return refuse(
                StatusCode::UNPROCESSABLE_ENTITY,
                "settings.err.current",
                ctx,
                tokens,
            );
        }
        Err(error) => return server_error(ctx, &error),
    }
    if new.chars().count() < MIN_PASSWORD_LEN {
        return render_with(
            StatusCode::UNPROCESSABLE_ENTITY,
            &page_with(
                &state,
                ctx,
                None,
                Some(t.t1("settings.err.short", "n", MIN_PASSWORD_LEN)),
                tokens,
            ),
        );
    }
    if new != again {
        return refuse(
            StatusCode::UNPROCESSABLE_ENTITY,
            "settings.err.mismatch",
            ctx,
            tokens,
        );
    }
    state.limiter.reset();
    if let Err(error) = change_password(&state.store, new).await {
        return server_error(ctx, &error);
    }
    // Whoever knew the old password may be signed in somewhere: those sessions end, this one
    // stays.
    let keep = key.map(|Extension(SessionKey(key))| key);
    if let Err(error) = state
        .store
        .delete_sessions_except(keep.as_ref().map(<[u8; 32]>::as_slice))
        .await
    {
        return server_error(ctx, &error);
    }
    Redirect::to(&ctx.url("/settings?done=password")).into_response()
}

/// `POST /settings/sign-out-everywhere`: end every session, this one too.
pub(crate) async fn sign_out_everywhere(State(state): State<AppState>, ctx: Ctx) -> Response {
    if state.web.auth != Auth::Password {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Err(error) = state.store.delete_sessions_except(None).await {
        return server_error(ctx, &error);
    }
    let mut response = Redirect::to(&login_url(&state.web.base_path(), None)).into_response();
    if let Some(cookie) = clear_cookie(&state.web) {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    response
}
