//! Signing in and out.

use std::time::Instant;

use askama::Template;
use axum::Form;
use axum::extract::{Extension, Query, State};
use axum::http::header::{RETRY_AFTER, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use vigil_store::now_ms;

use crate::AppState;
use crate::auth::login_url;
use crate::auth::session::{self, LIFETIME_MS};
use crate::auth::{SessionKey, verify_admin_password};
use crate::config::Auth;
use crate::page::{Ctx, render_with};

#[derive(Template)]
#[template(path = "login.html")]
struct Login {
    ctx: Ctx,
    next: String,
    error: Option<&'static str>,
}

/// Where the sign-in page was asked to send the visitor afterwards.
#[derive(Debug, Deserialize)]
pub(crate) struct NextQuery {
    #[serde(default)]
    next: String,
}

/// The sign-in form.
#[derive(Debug, Deserialize)]
pub(crate) struct LoginForm {
    #[serde(default)]
    password: String,
    #[serde(default)]
    next: String,
}

/// `GET /login`.
pub(crate) async fn login_form(
    State(state): State<AppState>,
    ctx: Ctx,
    headers: HeaderMap,
    Query(query): Query<NextQuery>,
) -> Response {
    let next = crate::auth::safe_next(&state.web.base_path(), &query.next).unwrap_or_default();
    let signed_in = state.web.auth == Auth::Disabled
        || matches!(
            crate::auth::current_session(&state, &headers).await,
            Ok(Some(_))
        );
    if signed_in {
        return Redirect::to(&landing(&state, &next)).into_response();
    }
    render_with(
        StatusCode::OK,
        &Login {
            ctx,
            next,
            error: None,
        },
    )
}

/// `POST /login`.
pub(crate) async fn login_submit(
    State(state): State<AppState>,
    ctx: Ctx,
    Form(form): Form<LoginForm>,
) -> Response {
    let next = crate::auth::safe_next(&state.web.base_path(), &form.next).unwrap_or_default();
    if state.web.auth == Auth::Disabled {
        return Redirect::to(&landing(&state, &next)).into_response();
    }
    let failed = |status: StatusCode, error: &'static str, ctx: Ctx| {
        render_with(
            status,
            &Login {
                ctx,
                next: next.clone(),
                error: Some(error),
            },
        )
    };

    if let Some(wait) = state.limiter.wait(Instant::now()) {
        let mut response = failed(StatusCode::TOO_MANY_REQUESTS, "login.too_many", ctx);
        if let Ok(seconds) = wait.as_secs().max(1).to_string().parse() {
            response.headers_mut().insert(RETRY_AFTER, seconds);
        }
        return response;
    }

    match verify_admin_password(&state.store, &form.password).await {
        Ok(true) => {}
        Ok(false) => {
            state.limiter.record_failure(Instant::now());
            // A guesser gets no faster for a wrong answer than for a right one.
            tokio::time::sleep(state.web.failed_login_delay).await;
            return failed(StatusCode::UNAUTHORIZED, "login.wrong", ctx);
        }
        Err(error) => {
            tracing::error!(%error, "checking the password failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    state.limiter.reset();
    let (Ok(token), now) = (session::new_token(), now_ms()) else {
        tracing::error!("no source of random numbers for a session");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let created = async {
        state.store.purge_expired_sessions(now).await?;
        state
            .store
            .create_session(&session::hash_token(&token), now, now + LIFETIME_MS)
            .await
    }
    .await;
    if let Err(error) = created {
        tracing::error!(%error, "starting a session failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let mut response = Redirect::to(&landing(&state, &next)).into_response();
    if let Some(cookie) = session::set_cookie(&state.web, &token) {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    response
}

/// `POST /logout`.
pub(crate) async fn logout(
    State(state): State<AppState>,
    key: Option<Extension<SessionKey>>,
) -> Response {
    if let Some(Extension(SessionKey(key))) = key
        && let Err(error) = state.store.delete_session(&key).await
    {
        tracing::error!(%error, "ending a session failed");
    }
    let mut response = Redirect::to(&login_url(&state.web.base_path(), None)).into_response();
    if let Some(cookie) = session::clear_cookie(&state.web) {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    response
}

/// Where to go after signing in: back to where the visitor was, or the home page.
fn landing(state: &AppState, next: &str) -> String {
    if next.is_empty() {
        let base = state.web.base_path();
        if base.is_empty() {
            "/".to_owned()
        } else {
            format!("{base}/")
        }
    } else {
        next.to_owned()
    }
}
