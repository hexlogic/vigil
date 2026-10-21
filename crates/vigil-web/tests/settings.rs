//! The settings page: language, password, sessions.

mod common;

use axum::http::{StatusCode, header};
use vigil_web::{Auth, WebConfig};

use common::*;

fn form(fields: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish()
}

async fn session_count(app: &App) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(app.store.pool())
        .await
}

#[tokio::test]
async fn the_page_shows_the_language_the_sessions_and_the_facts() -> TestResult {
    let app = app().await?;
    let response = app.get("/settings").await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.body();
    assert!(body.contains("<h1>Settings</h1>"), "{body}");
    assert!(body.contains("aria-current=\"page\">Settings<"), "{body}");
    assert!(
        body.contains("<option value=\"en\" selected>English</option>"),
        "{body}"
    );
    assert!(
        body.contains("<option value=\"uk\">Українська</option>"),
        "{body}"
    );
    assert!(
        body.contains("action=\"/settings/password\"")
            && body.contains("action=\"/settings/sign-out-everywhere\""),
        "{body}"
    );
    assert!(body.contains(env!("CARGO_PKG_VERSION")), "{body}");
    assert!(
        body.contains("base_url is not set, so notifications carry no link"),
        "{body}"
    );
    assert!(!body.contains(" style="));

    let named = app_with(WebConfig::new(
        Auth::Password,
        Some("https://vigil.example.com"),
    ))
    .await?;
    let body = named.get("/settings").await?.into_body();
    assert!(
        body.contains("https://vigil.example.com") && !body.contains("base_url is not set"),
        "{body}"
    );
    Ok(())
}

#[tokio::test]
async fn a_notice_only_shows_for_the_things_it_knows() -> TestResult {
    let app = app().await?;
    assert!(
        app.get("/settings?done=language")
            .await?
            .body()
            .contains("The language was changed.")
    );
    assert!(
        app.get("/settings?done=password")
            .await?
            .body()
            .contains("The password was changed.")
    );
    let other = app.get("/settings?done=%3Cb%3Ehello").await?.into_body();
    assert!(
        !other.contains("hello") && !other.contains("notice-ok"),
        "{other}"
    );
    Ok(())
}

#[tokio::test]
async fn the_language_is_remembered_in_a_cookie() -> TestResult {
    let app = app().await?;
    let response = app.post_form("/settings/language", "lang=uk").await?;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(header_of(&response, "location"), "/settings?done=language");
    let cookie = header_of(&response, "set-cookie");
    assert!(
        cookie.starts_with("vigil_lang=uk; Path=/; Max-Age=31536000"),
        "{cookie}"
    );
    assert!(
        cookie.contains("SameSite=Lax") && !cookie.contains("Secure"),
        "{cookie}"
    );

    // With the cookie, the pages are in Ukrainian whatever the browser asks for.
    let browser = App {
        cookie: app.cookie.clone().map(|c| format!("{c}; vigil_lang=uk")),
        ..app.clone()
    };
    let body = browser.get("/settings").await?.into_body();
    assert!(
        body.contains("<html lang=\"uk\">") && body.contains("Налаштування"),
        "{body}"
    );
    assert!(body.contains("<option value=\"uk\" selected>"), "{body}");

    assert_eq!(
        app.post_form("/settings/language", "lang=xx")
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.post_form("/settings/language", "").await?.status(),
        StatusCode::BAD_REQUEST
    );

    let secure = app_with(WebConfig::new(
        Auth::Password,
        Some("https://example.com/vigil"),
    ))
    .await?;
    let response = secure
        .post_form("/vigil/settings/language", "lang=en")
        .await?;
    let cookie = header_of(&response, "set-cookie");
    assert!(
        cookie.contains("Path=/vigil;") && cookie.ends_with("; Secure"),
        "{cookie}"
    );
    Ok(())
}

#[tokio::test]
async fn changing_the_password_signs_the_other_browsers_out_and_keeps_this_one() -> TestResult {
    let app = app().await?;
    let other = App {
        cookie: Some(app.sign_in().await?),
        ..app.clone()
    };
    assert_eq!(session_count(&app).await?, 2);

    let response = app
        .post_form(
            "/settings/password",
            &form(&[
                ("current", PASSWORD),
                ("new", "a brand new password"),
                ("again", "a brand new password"),
            ]),
        )
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "{}",
        response.body()
    );
    assert_eq!(header_of(&response, "location"), "/settings?done=password");

    assert_eq!(session_count(&app).await?, 1);
    assert_eq!(
        app.get("/settings").await?.status(),
        StatusCode::OK,
        "this browser stays signed in"
    );
    assert_eq!(
        other.get("/settings").await?.status(),
        StatusCode::SEE_OTHER,
        "the other one does not"
    );

    let guest = app.signed_out();
    assert_eq!(
        guest
            .post_form("/login", "password=correct+horse+battery")
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        guest
            .post_form("/login", "password=a+brand+new+password")
            .await?
            .status(),
        StatusCode::SEE_OTHER
    );
    Ok(())
}

#[tokio::test]
async fn a_bad_password_change_changes_nothing() -> TestResult {
    let app = app().await?;
    let cases = [
        (
            form(&[
                ("current", "wrong"),
                ("new", "long enough password"),
                ("again", "long enough password"),
            ]),
            "That is not the current password.",
        ),
        (
            form(&[("current", PASSWORD), ("new", "short"), ("again", "short")]),
            "at least 8 characters",
        ),
        (
            form(&[
                ("current", PASSWORD),
                ("new", "long enough password"),
                ("again", "another one entirely"),
            ]),
            "The two new passwords are not the same.",
        ),
    ];
    for (body, message) in cases {
        let response = app.post_form("/settings/password", &body).await?;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{message}"
        );
        assert!(
            response.body().contains(message),
            "{message} in {}",
            response.body()
        );
        assert!(
            !response.body().contains("long enough password"),
            "passwords are not echoed"
        );
    }
    assert_eq!(
        app.signed_out()
            .post_form("/login", "password=correct+horse+battery")
            .await?
            .status(),
        StatusCode::SEE_OTHER
    );
    Ok(())
}

#[tokio::test]
async fn guessing_the_current_password_here_is_limited_like_signing_in() -> TestResult {
    let app = app().await?;
    for _ in 0..5 {
        let wrong = app
            .post_form(
                "/settings/password",
                &form(&[
                    ("current", "nope"),
                    ("new", "long enough password"),
                    ("again", "long enough password"),
                ]),
            )
            .await?;
        assert_eq!(wrong.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    let locked = app
        .post_form(
            "/settings/password",
            &form(&[
                ("current", PASSWORD),
                ("new", "long enough password"),
                ("again", "long enough password"),
            ]),
        )
        .await?;
    assert_eq!(locked.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(locked.headers().get(header::RETRY_AFTER).is_some());
    Ok(())
}

#[tokio::test]
async fn a_password_from_the_environment_cannot_be_changed_here() -> TestResult {
    let mut web = WebConfig::new(Auth::Password, None);
    web.password_from_env = true;
    let app = app_with(web).await?;
    let body = app.get("/settings").await?.into_body();
    assert!(body.contains("set by VIGIL_ADMIN_PASSWORD"), "{body}");
    assert!(!body.contains("action=\"/settings/password\""), "{body}");
    let response = app
        .post_form(
            "/settings/password",
            &form(&[
                ("current", PASSWORD),
                ("new", "long enough password"),
                ("again", "long enough password"),
            ]),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn without_sign_in_there_is_no_password_or_session_to_manage() -> TestResult {
    let app = app_with(WebConfig::new(Auth::Disabled, None)).await?;
    let body = app.get("/settings").await?.into_body();
    assert!(
        !body.contains("/settings/password") && !body.contains("sign-out-everywhere"),
        "{body}"
    );
    assert_eq!(
        app.post_form(
            "/settings/password",
            &form(&[("current", "x"), ("new", "y"), ("again", "y")])
        )
        .await?
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.post_form("/settings/sign-out-everywhere", "")
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    // The language still can be chosen.
    assert_eq!(
        app.post_form("/settings/language", "lang=uk")
            .await?
            .status(),
        StatusCode::SEE_OTHER
    );
    Ok(())
}

#[tokio::test]
async fn signing_out_everywhere_ends_every_session_this_one_too() -> TestResult {
    let app = app().await?;
    let other = App {
        cookie: Some(app.sign_in().await?),
        ..app.clone()
    };
    assert_eq!(session_count(&app).await?, 2);

    let response = app.post_form("/settings/sign-out-everywhere", "").await?;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(header_of(&response, "location"), "/login");
    assert!(header_of(&response, "set-cookie").contains("Max-Age=0"));
    assert_eq!(session_count(&app).await?, 0);
    assert_eq!(app.get("/settings").await?.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        other.get("/settings").await?.status(),
        StatusCode::SEE_OTHER
    );
    Ok(())
}

#[tokio::test]
async fn settings_need_a_session_and_a_request_from_this_site() -> TestResult {
    let app = app().await?;
    let guest = app.signed_out();
    assert_eq!(
        guest.get("/settings").await?.status(),
        StatusCode::SEE_OTHER
    );
    for path in [
        "/settings/language",
        "/settings/password",
        "/settings/sign-out-everywhere",
    ] {
        assert_eq!(
            guest.post_form(path, "lang=uk").await?.status(),
            StatusCode::SEE_OTHER,
            "{path}"
        );
        assert_eq!(
            app.post_form_from(path, "lang=uk", "https://evil.example")
                .await?
                .status(),
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    assert_eq!(session_count(&app).await?, 1, "the session survived");
    Ok(())
}
