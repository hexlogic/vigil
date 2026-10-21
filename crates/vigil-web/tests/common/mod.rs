//! Shared setup for the web tests.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, Response, header};
use http_body_util::BodyExt;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use vigil_core::Schedule;
use vigil_fetch::{FetchConfig, Fetcher};
use vigil_notify::{Sender, SenderConfig};
use vigil_scheduler::{Checker, Notifier, Scheduler, Settings};
use vigil_store::{NewWatch, Store};
use vigil_web::{AppState, Auth, WebConfig, ensure_admin_password, router};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The admin password of every test app.
pub const PASSWORD: &str = "correct horse battery";

/// Stops the scheduler of a test app when the app is dropped.
pub struct Stop(CancellationToken);

impl Drop for Stop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// A router on a throwaway database, with a running scheduler and a browser that may be
/// signed in.
#[derive(Clone)]
pub struct App {
    pub _dir: Arc<tempfile::TempDir>,
    pub _stop: Arc<Stop>,
    pub router: Router,
    pub store: Store,
    pub checker: Checker,
    pub notifier: Arc<Notifier>,
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

pub async fn anonymous_with(web: WebConfig) -> Result<App, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("vigil.db")).await?;
    ensure_admin_password(&store, Some(PASSWORD)).await?;
    app_on(store, dir, web).await
}

/// An app on this database, with a running scheduler. Nobody is signed in.
pub async fn app_on(
    store: Store,
    dir: tempfile::TempDir,
    mut web: WebConfig,
) -> Result<App, Box<dyn std::error::Error>> {
    // Waiting a second for every wrong password would only slow the tests down.
    web.failed_login_delay = Duration::ZERO;
    let base_path = web.base_path();
    let origin = web
        .origin()
        .unwrap_or_else(|| "http://localhost".to_owned());

    let checker = Checker::new(store.clone(), Fetcher::new(FetchConfig::default())?);
    let notifier = Arc::new(
        Notifier::new(store.clone(), Sender::new(SenderConfig::default())?)
            .with_base_url(web.base_url.clone())
            .collecting_reports(),
    );
    let checker = checker.with_notifier(Arc::clone(&notifier));
    let scheduler = Scheduler::new(checker.clone(), Settings::default());
    let handle = scheduler.handle();
    let stop = CancellationToken::new();
    let token = stop.clone();
    tokio::spawn(async move { scheduler.run(token).await });

    let router = router(AppState::new(
        store.clone(),
        web,
        checker.clone(),
        Arc::clone(&notifier),
        handle,
    )?);
    Ok(App {
        _dir: Arc::new(dir),
        _stop: Arc::new(Stop(stop)),
        router,
        store,
        checker,
        notifier,
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

    /// A watch of `url` that is checked every hour.
    pub async fn add_watch(
        &self,
        name: &str,
        url: &str,
    ) -> Result<vigil_core::Watch, Box<dyn std::error::Error>> {
        let mut new = NewWatch::new(url, Schedule::Interval(3600));
        new.name = name.to_owned();
        Ok(self.store.create_watch(new).await?)
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

    /// A new API token, as a script would be given it.
    pub async fn api_token(&self, name: &str) -> Result<String, Box<dyn std::error::Error>> {
        let made = vigil_web::generate_token()?;
        self.store
            .create_api_token(name, &made.hash, &made.prefix)
            .await?;
        Ok(made.token)
    }

    /// A call to the API: no cookie, the token if there is one, a JSON body if there is one.
    /// It is sent as a command-line client would, with neither `Origin` nor `Sec-Fetch-Site`.
    pub async fn api(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<&serde_json::Value>,
    ) -> Result<(axum::http::StatusCode, serde_json::Value), Box<dyn std::error::Error>> {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            request = request.header("x-api-key", token);
        }
        let request = match body {
            Some(body) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))?,
            None => request.body(Body::empty())?,
        };
        let response = self.signed_out().send(request).await?;
        let status = response.status();
        let json = serde_json::from_str(response.body())
            .map_err(|e| format!("not JSON ({e}): {} {:?}", status, response.body()))?;
        Ok((status, json))
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
