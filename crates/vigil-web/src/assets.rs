//! The CSS, JavaScript and icons that are built into the binary.

use axum::extract::{Path, Query};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;
use std::collections::HashMap;

/// Everything in the repository's `assets/` folder. `debug-embed` makes debug builds embed
/// the files too, so tests see what the release binary serves.
#[derive(RustEmbed)]
#[folder = "../../assets/"]
struct Assets;

/// Long-lived caching for a URL that carries the version (`?v=...`), which changes with the
/// release; revalidation for one that does not.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const REVALIDATE: &str = "no-cache";

/// `GET /assets/{name}`.
pub(crate) async fn serve(
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let Some(file) = Assets::get(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let etag = format!("\"{}\"", hex(&file.metadata.sha256_hash()));
    let cache = if query.contains_key("v") {
        IMMUTABLE
    } else {
        REVALIDATE
    };
    let unchanged = headers
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|sent| sent.split(',').any(|tag| tag.trim() == etag));

    let mut response = if unchanged {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        file.data.into_owned().into_response()
    };
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Ok(value) = HeaderValue::from_str(&etag) {
        headers.insert(ETAG, value);
    }
    if !unchanged {
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type(&name)));
    }
    response
}

/// The media type for a file name, by extension. Only the kinds of files in `assets/`.
fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, extension)| extension) {
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("txt" | "LICENSE") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stylesheets_and_scripts_stay_under_100_kb() {
        let bytes: usize = Assets::iter()
            .filter(|name| name.ends_with(".css") || name.ends_with(".js"))
            .filter_map(|name| Assets::get(&name))
            .map(|file| file.data.len())
            .sum();
        assert!(bytes < 100 * 1024, "{bytes} bytes of css and js");
    }

    /// htmx is vendored as published, not edited. The hash is the one of `dist/htmx.min.js`
    /// in the htmx.org 2.0.11 package (see docs/decisions.md); change both together.
    #[test]
    fn the_vendored_htmx_is_the_published_file() {
        let file = Assets::get("htmx.min.js").expect("htmx is embedded");
        assert_eq!(
            hex(&file.metadata.sha256_hash()),
            "d6fdc75f204e6bdefa99b69bf1e6d4ac69b8a364f77929f45c13476b4000f717"
        );
        assert!(
            Assets::get("htmx.LICENSE").is_some(),
            "its licence ships with it"
        );
    }

    #[test]
    fn media_types_follow_the_extension() {
        assert_eq!(content_type("vigil.css"), "text/css; charset=utf-8");
        assert_eq!(
            content_type("htmx.min.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type("icon.svg"), "image/svg+xml");
        assert_eq!(content_type("htmx.LICENSE"), "text/plain; charset=utf-8");
        assert_eq!(content_type("data.bin"), "application/octet-stream");
        assert_eq!(content_type("noextension"), "application/octet-stream");
    }
}
