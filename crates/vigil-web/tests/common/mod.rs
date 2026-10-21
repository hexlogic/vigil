//! Shared setup for the web tests.

#![allow(dead_code)]

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, header};
use http_body_util::BodyExt;
use tower::ServiceExt;
use vigil_store::Store;
use vigil_web::{AppState, Auth, WebConfig, ensure_admin_password, router};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The admin password of every test app.
pub const PASSWORD: &str = "correct horse battery";

/// A router on a throwaway database, with a browser that may be signed in.
#[derive(Clone)]
pub struct App {
    pub _dir: std::sync::Arc<tempfile::TempDir>,
    pub router: Router,
    pub store: Store,
    /// The session cookie the "browser" sends, if it is signed in.
    pub cookie: Option<String>,
    /// The path the UI is served under, and the origin pages of it are from.
    pub base_path: String,
    pub origin: String,
}

/// An app with sign-in on, and a browser that is signed in.
pub async fn app() -> Result<App, Box<dyn std::error::Error>> {
    app_with(WebConfig::new(Auth::Password, None)).await
}

/// An app with sign-in on, and a browser that has not signed in.
pub async fn anonymous() -> Result<App, Box<dyn std::error::Error>> {
    anonymous_with(WebConfig::new(Auth::Password, None)).await
}

/// An app with these settings; signed in if they ask for sign-in.
pub async fn app_with(web: WebConfig) -> Result<App, Box<dyn std::error::Error>> {
    let signs_in = web.auth == Auth::Password;
    let mut app = anonymous_with(web).await?;
    if signs_in {
        app.cookie = Some(app.sign_in().await?);
    }
    Ok(app)
}

pub async fn anonymous_with(mut web: WebConfig) -> Result<App, Box<dyn std::error::Error>> {
    // Waiting a second for every wrong password would only slow the tests down.
    web.failed_login_delay = Duration::ZERO;
    let base_path = web.base_path();
    let origin = web
        .origin()
        .unwrap_or_else(|| "http://localhost".to_owned());
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("vigil.db")).await?;
    ensure_admin_password(&store, Some(PASSWORD)).await?;
    let router = router(AppState::new(store.clone(), web)?);
    Ok(App {
        _dir: std::sync::Arc::new(dir),
        router,
        store,
        cookie: None,
        base_path,
        origin,
    })
}

impl App {
    /// A browser that has not signed in, using the same server.
    pub fn signed_out(&self) -> App {
        App {
            cookie: None,
            ..self.clone()
        }
    }

    /// Sign in and return the session cookie (`vigil_session=...`).
    pub async fn sign_in(&self) -> Result<String, Box<dyn std::error::Error>> {
        let response = self
            .post_form(
                &format!("{}/login", self.base_path),
                &format!("password={}", PASSWORD.replace(' ', "+")),
            )
            .await?;
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .ok_or_else(|| {
                format!(
                    "signing in set no cookie: {} {:?}",
                    response.status(),
                    response.body()
                )
            })?
            .to_owned();
        Ok(cookie)
    }

    /// Send a request and read the whole answer. The request is sent to `localhost` unless it
    /// names a host, and carries the session cookie if the browser has one.
    pub async fn send(
        &self,
        mut request: Request<Body>,
    ) -> Result<Response<String>, Box<dyn std::error::Error>> {
        if !request.headers().contains_key(header::HOST) {
            request
                .headers_mut()
                .insert(header::HOST, "localhost".parse()?);
        }
        if let Some(cookie) = &self.cookie {
            let joined = match request.headers().get(header::COOKIE) {
                Some(existing) => format!("{}; {cookie}", existing.to_str()?),
                None => cookie.clone(),
            };
            request
                .headers_mut()
                .insert(header::COOKIE, joined.parse()?);
        }
        let response = self.router.clone().oneshot(request).await?;
        let (parts, body) = response.into_parts();
        let bytes = body.collect().await?.to_bytes();
        Ok(Response::from_parts(
            parts,
            String::from_utf8_lossy(&bytes).into_owned(),
        ))
    }

    /// `GET path`.
    pub async fn get(&self, path: &str) -> Result<Response<String>, Box<dyn std::error::Error>> {
        self.send(Request::get(path).body(Body::empty())?).await
    }

    /// `GET path` asking for `language`.
    pub async fn get_in(
        &self,
        path: &str,
        language: &str,
    ) -> Result<Response<String>, Box<dyn std::error::Error>> {
        self.send(
            Request::get(path)
                .header(header::ACCEPT_LANGUAGE, language)
                .body(Body::empty())?,
        )
        .await
    }

    /// `POST path` with a form body, as a page of this site would send it.
    pub async fn post_form(
        &self,
        path: &str,
        body: &str,
    ) -> Result<Response<String>, Box<dyn std::error::Error>> {
        self.post_form_from(path, body, &self.origin).await
    }

    /// `POST path` with a form body, from a page at `origin`.
    pub async fn post_form_from(
        &self,
        path: &str,
        body: &str,
        origin: &str,
    ) -> Result<Response<String>, Box<dyn std::error::Error>> {
        self.send(
            Request::post(path)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::ORIGIN, origin)
                .body(Body::from(body.to_owned()))?,
        )
        .await
    }
}

/// A header's value, or an empty string.
pub fn header_of<'a>(response: &'a Response<String>, name: &str) -> &'a str {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
}
