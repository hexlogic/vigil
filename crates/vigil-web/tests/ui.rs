//! The page layout, languages, assets and the headers every answer carries.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use vigil_core::Schedule;
use vigil_store::NewWatch;
use vigil_web::{Auth, WebConfig};

use common::*;

#[tokio::test]
async fn the_home_page_is_laid_out_in_english_by_default() -> TestResult {
    let app = app().await?;
    let response = app.get("/").await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(header_of(&response, "content-type").starts_with("text/html"));
    let body = response.body();
    assert!(body.contains("<html lang=\"en\">"), "{body}");
    assert!(body.contains("<title>Watches · Vigil</title>"), "{body}");
    assert!(body.contains("No watches yet."), "{body}");
    for entry in ["Watches", "Notifications", "Tags", "Settings"] {
        assert!(body.contains(&format!(">{entry}</a>")), "{entry} in {body}");
    }
    assert!(
        body.contains("aria-current=\"page\""),
        "the current entry is marked"
    );
    Ok(())
}

#[tokio::test]
async fn the_page_follows_the_language_of_the_request() -> TestResult {
    let app = app().await?;
    let uk = app.get_in("/", "uk-UA,uk;q=0.9,en;q=0.5").await?;
    assert!(uk.body().contains("<html lang=\"uk\">"));
    assert!(uk.body().contains("Відстежень ще немає."), "{}", uk.body());
    assert!(uk.body().contains(">Налаштування</a>"));

    // The language cookie beats the header.
    let cookie = app
        .send(
            Request::get("/")
                .header(header::ACCEPT_LANGUAGE, "uk")
                .header(header::COOKIE, "vigil_lang=en")
                .body(Body::empty())?,
        )
        .await?;
    assert!(cookie.body().contains("<html lang=\"en\">"));

    // A language we do not have falls back to English.
    let de = app.get_in("/", "de").await?;
    assert!(de.body().contains("No watches yet."));
    Ok(())
}

#[tokio::test]
async fn counts_use_the_plural_form_of_the_language() -> TestResult {
    let app = app().await?;
    for url in [
        "https://a.example",
        "https://b.example",
        "https://c.example",
    ] {
        app.store
            .create_watch(NewWatch::new(url, Schedule::Interval(60)))
            .await?;
    }
    assert!(app.get("/").await?.body().contains("3 watches"));
    let uk = app.get_in("/", "uk").await?;
    assert!(uk.body().contains("3 відстеження"), "{}", uk.body());
    Ok(())
}

#[tokio::test]
async fn an_unknown_address_gets_a_not_found_page() -> TestResult {
    let app = app().await?;
    let response = app.get("/no/such/page").await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(response.body().contains("Page not found"));
    let uk = app.get_in("/no/such/page", "uk").await?;
    assert!(uk.body().contains("Сторінку не знайдено"));
    Ok(())
}

#[tokio::test]
async fn assets_are_served_with_their_type_and_cache_headers() -> TestResult {
    let app = app().await?;
    let css = app.get("/assets/vigil.css").await?;
    assert_eq!(css.status(), StatusCode::OK);
    assert_eq!(header_of(&css, "content-type"), "text/css; charset=utf-8");
    assert_eq!(header_of(&css, "cache-control"), "no-cache");
    assert!(css.body().contains("prefers-color-scheme"));

    let versioned = app.get("/assets/vigil.css?v=1.2.3").await?;
    assert_eq!(
        header_of(&versioned, "cache-control"),
        "public, max-age=31536000, immutable"
    );

    let js = app.get("/assets/htmx.min.js").await?;
    assert_eq!(
        header_of(&js, "content-type"),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(app.get("/assets/icon.svg").await?.status(), StatusCode::OK);
    assert_eq!(
        app.get("/assets/nothing.css").await?.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get("/assets/..%2Fvigil.db").await?.status(),
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn an_asset_that_has_not_changed_is_not_sent_again() -> TestResult {
    let app = app().await?;
    let first = app.get("/assets/vigil.css").await?;
    let etag = header_of(&first, "etag").to_owned();
    assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");

    let again = app
        .send(
            Request::get("/assets/vigil.css")
                .header(header::IF_NONE_MATCH, &etag)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
    assert!(again.body().is_empty());
    assert_eq!(header_of(&again, "etag"), etag);

    let other = app
        .send(
            Request::get("/assets/vigil.css")
                .header(header::IF_NONE_MATCH, "\"something-else\"")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(other.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn pages_and_assets_carry_the_security_headers() -> TestResult {
    let app = app().await?;
    for path in ["/", "/assets/vigil.css", "/no/such/page", "/healthz"] {
        let response = app.get(path).await?;
        let csp = header_of(&response, "content-security-policy");
        assert!(csp.contains("default-src 'self'"), "{path}: {csp}");
        assert!(csp.contains("frame-ancestors 'none'"), "{path}: {csp}");
        assert!(!csp.contains("unsafe"), "{path}: {csp}");
        assert_eq!(
            header_of(&response, "x-content-type-options"),
            "nosniff",
            "{path}"
        );
        assert_eq!(header_of(&response, "x-frame-options"), "DENY", "{path}");
        assert_eq!(
            header_of(&response, "referrer-policy"),
            "same-origin",
            "{path}"
        );
    }
    // Pages are never cached; an asset keeps its own policy.
    assert_eq!(header_of(&app.get("/").await?, "cache-control"), "no-store");
    assert_eq!(
        header_of(&app.get("/assets/vigil.css").await?, "cache-control"),
        "no-cache"
    );
    Ok(())
}

#[tokio::test]
async fn the_layout_loads_only_its_own_files_and_sets_up_htmx_for_the_policy() -> TestResult {
    let app = app().await?;
    let body = app.get("/").await?.into_body();
    assert!(body.contains("href=\"/assets/vigil.css?v="), "{body}");
    assert!(body.contains("src=\"/assets/htmx.min.js?v="), "{body}");
    assert!(body.contains("\"includeIndicatorStyles\":false"));
    assert!(body.contains("\"allowEval\":false"));
    assert!(!body.contains("<style"), "no inline styles");
    assert!(!body.contains(" style="), "no style attributes");
    assert!(!body.contains("<script>"), "no inline scripts");
    assert!(!body.contains(" onclick="), "no inline handlers");
    Ok(())
}

#[tokio::test]
async fn the_answers_are_compressed_when_the_client_accepts_it() -> TestResult {
    let app = app().await?;
    let plain = app.get("/assets/vigil.css").await?;
    assert_eq!(header_of(&plain, "content-encoding"), "");
    let gzipped = app
        .send(
            Request::get("/assets/vigil.css")
                .header(header::ACCEPT_ENCODING, "gzip")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(header_of(&gzipped, "content-encoding"), "gzip");
    Ok(())
}

#[tokio::test]
async fn turning_sign_in_off_is_announced_on_every_page() -> TestResult {
    let on = app().await?;
    assert!(!on.get("/").await?.body().contains("Sign-in is off"));
    let off = app_with(WebConfig::new(Auth::Disabled, None)).await?;
    assert!(off.get("/").await?.body().contains("Sign-in is off"));
    assert!(
        off.get("/no/such/page")
            .await?
            .body()
            .contains("Sign-in is off")
    );
    Ok(())
}

#[tokio::test]
async fn the_ui_lives_under_the_path_of_the_base_url() -> TestResult {
    let app = app_with(WebConfig::new(
        Auth::Password,
        Some("https://example.com/vigil/"),
    ))
    .await?;

    // The address with a final slash is sent on to the one without.
    let slash = app.get("/vigil/").await?;
    assert_eq!(slash.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(header_of(&slash, "location"), "/vigil");
    let home = app.get("/vigil").await?;
    assert_eq!(home.status(), StatusCode::OK);
    let body = home.body();
    assert!(body.contains("href=\"/vigil/assets/vigil.css?v="), "{body}");
    assert!(body.contains("href=\"/vigil/notifications\""), "{body}");
    assert_eq!(
        app.get("/vigil/assets/vigil.css").await?.status(),
        StatusCode::OK
    );

    // Outside the prefix there is nothing, except the health check.
    assert_eq!(app.get("/").await?.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        app.get("/assets/vigil.css").await?.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(app.get("/healthz").await?.status(), StatusCode::OK);
    // Inside it, an unknown page still gets the friendly page.
    let missing = app.get("/vigil/nothing").await?;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert!(missing.body().contains("Page not found"));
    Ok(())
}

#[tokio::test]
async fn asset_links_carry_a_fingerprint_of_the_file() -> TestResult {
    let app = app().await?;
    let body = app.get("/").await?.into_body();
    let link = body
        .split('"')
        .find(|part| part.starts_with("/assets/vigil.css?v="))
        .ok_or("no stylesheet link")?;
    let fingerprint = link.rsplit('=').next().ok_or("no fingerprint")?;
    assert_eq!(fingerprint.len(), 10, "{link}");
    // It is the start of the file's own hash, which is also its ETag.
    let etag = header_of(&app.get("/assets/vigil.css").await?, "etag")
        .trim_matches('"')
        .to_owned();
    assert!(etag.starts_with(fingerprint), "{etag} vs {fingerprint}");
    // The link is cached for a year, and the file behind it is the one it names.
    let served = app.get(link).await?;
    assert_eq!(
        header_of(&served, "cache-control"),
        "public, max-age=31536000, immutable"
    );
    Ok(())
}
