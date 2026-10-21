//! Shared setup for the web tests.

#![allow(dead_code)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, header};
use http_body_util::BodyExt;
use tower::ServiceExt;
use vigil_store::Store;
use vigil_web::{AppState, Auth, WebConfig, router};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A router on a throwaway database.
pub struct App {
    pub _dir: tempfile::TempDir,
    pub router: Router,
    pub store: Store,
}

pub async fn app() -> Result<App, Box<dyn std::error::Error>> {
    app_with(WebConfig::new(Auth::Password, None)).await
}

pub async fn app_with(web: WebConfig) -> Result<App, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("vigil.db")).await?;
    let router = router(AppState::new(store.clone(), web)?);
    Ok(App {
        _dir: dir,
        router,
        store,
    })
}

impl App {
    /// Send a request and read the whole answer.
    pub async fn send(
        &self,
        request: Request<Body>,
    ) -> Result<Response<String>, Box<dyn std::error::Error>> {
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
}

/// A header's value, or an empty string.
pub fn header_of<'a>(response: &'a Response<String>, name: &str) -> &'a str {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
}
