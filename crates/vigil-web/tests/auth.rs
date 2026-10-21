//! Signing in and out, sessions, the admin password and the checks on where a request is from.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use vigil_store::now_ms;
use vigil_web::{Auth, PasswordSource, WebConfig, ensure_admin_password};

use common::*;

fn location(response: &axum::http::Response<String>) -> &str {
    header_of(response, "location")
}

#[tokio::test]
async fn a_page_needs_a_session_and_the_login_brings_the_visitor_back() -> TestResult {
    let app = anonymous().await?;
    let response = app.get("/").await?;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/login");

    let nested = app.get("/watch/1?tab=x").await?;
    // An unknown address is not behind the login: it is just not found.
    assert_eq!(nested.status(), StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn a_request_made_by_htmx_is_told_to_go_to_the_login_instead_of_following_it() -> TestResult {
    let app = anonymous().await?;
    let fragment = app
        .send(
            Request::get("/")
                .header("hx-request", "true")
                .header("hx-current-url", "http://localhost/tags?page=2")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(fragment.status(), StatusCode::UNAUTHORIZED);
    // It comes back to the page the visitor was on, not to the fragment that was asked for.
    assert_eq!(
        header_of(&fragment, "hx-redirect"),
        "/login?next=%2Ftags%3Fpage%3D2"
    );

    let blind = app
        .send(
            Request::get("/")
                .header("hx-request", "true")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(header_of(&blind, "hx-redirect"), "/login");

    // A page on another site cannot be put there.
    let foreign = app
        .send(
            Request::get("/")
                .header("hx-request", "true")
                .header("hx-current-url", "http://localhost//evil.example")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(header_of(&foreign, "hx-redirect"), "/login");
    Ok(())
}

#[tokio::test]
async fn the_login_page_asks_for_a_password() -> TestResult {
    let app = anonymous().await?;
    let response = app.get("/login").await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.body();
    assert!(body.contains("<h1>Sign in</h1>"), "{body}");
    assert!(body.contains("type=\"password\""), "{body}");
    assert!(body.contains("autocomplete=\"current-password\""), "{body}");
    assert!(!body.contains(">Watches</a>"), "no menu before signing in");
    let uk = app.get_in("/login", "uk").await?;
    assert!(uk.body().contains("Пароль"));
    Ok(())
}

#[tokio::test]
async fn the_right_password_starts_a_session_and_the_wrong_one_does_not() -> TestResult {
    let app = anonymous().await?;

    let wrong = app.post_form("/login", "password=nope").await?;
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    assert!(wrong.body().contains("Wrong password."), "{}", wrong.body());
    assert!(wrong.headers().get(header::SET_COOKIE).is_none());
    assert_eq!(app.get("/").await?.status(), StatusCode::SEE_OTHER);

    let right = app
        .post_form("/login", "password=correct+horse+battery")
        .await?;
    assert_eq!(right.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&right), "/");
    let cookie = header_of(&right, "set-cookie").to_owned();
    assert!(cookie.starts_with("vigil_session="), "{cookie}");
    assert!(
        cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"),
        "{cookie}"
    );

    let signed_in = App {
        cookie: cookie.split(';').next().map(str::to_owned),
        ..app.clone()
    };
    assert_eq!(signed_in.get("/").await?.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn only_a_hash_of_the_session_token_is_stored() -> TestResult {
    let app = app().await?;
    let token = app
        .cookie
        .clone()
        .ok_or("no cookie")?
        .replace("vigil_session=", "");
    let rows = sqlx_hashes(&app).await?;
    assert_eq!(rows, [blake3::hash(token.as_bytes()).as_bytes().to_vec()]);
    Ok(())
}

async fn sqlx_hashes(app: &App) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error>> {
    Ok(sqlx::query_scalar("SELECT token_hash FROM sessions")
        .fetch_all(app.store.pool())
        .await?)
}

#[tokio::test]
async fn the_visitor_goes_back_to_where_they_were_but_never_to_another_site() -> TestResult {
    let app = anonymous().await?;
    let to = |next: &str| format!("password=correct+horse+battery&next={next}");

    let back = app.post_form("/login", &to("%2Fwatch%2F1%3Fa%3Db")).await?;
    assert_eq!(location(&back), "/watch/1?a=b");
    for evil in [
        "%2F%2Fevil.example",
        "https%3A%2F%2Fevil.example",
        "%2F%5Cevil.example",
        "evil.example",
    ] {
        let response = app.post_form("/login", &to(evil)).await?;
        assert_eq!(location(&response), "/", "{evil}");
    }
    // The login page does not carry a bad `next` into its form.
    let page = app.get("/login?next=//evil.example").await?;
    assert!(!page.body().contains("evil.example"), "{}", page.body());
    let page = app.get("/login?next=/tags").await?;
    assert!(
        page.body().contains("name=\"next\" value=\"/tags\""),
        "{}",
        page.body()
    );
    Ok(())
}

#[tokio::test]
async fn signing_in_twice_in_a_row_just_goes_home() -> TestResult {
    let app = app().await?;
    let response = app.get("/login").await?;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/");
    Ok(())
}

#[tokio::test]
async fn signing_out_ends_the_session_and_clears_the_cookie() -> TestResult {
    let app = app().await?;
    assert_eq!(sqlx_hashes(&app).await?.len(), 1);

    let response = app.post_form("/logout", "").await?;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/login");
    let cleared = header_of(&response, "set-cookie");
    assert!(
        cleared.starts_with("vigil_session=;") && cleared.contains("Max-Age=0"),
        "{cleared}"
    );

    assert!(
        sqlx_hashes(&app).await?.is_empty(),
        "the session is gone from the database"
    );
    // The old cookie no longer works, even if the browser keeps sending it.
    assert_eq!(app.get("/").await?.status(), StatusCode::SEE_OTHER);
    Ok(())
}

#[tokio::test]
async fn the_menu_offers_to_sign_out_only_when_there_is_a_sign_in() -> TestResult {
    let app = app().await?;
    let body = app.get("/").await?.into_body();
    assert!(body.contains("action=\"/logout\""), "{body}");
    assert!(body.contains(">Sign out</button>"), "{body}");

    let open = app_with(WebConfig::new(Auth::Disabled, None)).await?;
    assert!(!open.get("/").await?.body().contains("/logout"));
    Ok(())
}

#[tokio::test]
async fn too_many_wrong_passwords_lock_the_login_for_a_while() -> TestResult {
    let app = anonymous().await?;
    for _ in 0..5 {
        let wrong = app.post_form("/login", "password=nope").await?;
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    }
    // Even the right password waits.
    let locked = app
        .post_form("/login", "password=correct+horse+battery")
        .await?;
    assert_eq!(locked.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        locked.body().contains("Too many wrong passwords"),
        "{}",
        locked.body()
    );
    let wait: u64 = header_of(&locked, "retry-after").parse()?;
    assert!((1..=60).contains(&wait), "{wait}");
    assert!(locked.headers().get(header::SET_COOKIE).is_none());
    Ok(())
}

#[tokio::test]
async fn a_post_from_another_site_is_refused_before_anything_happens() -> TestResult {
    let app = app().await?;
    let sessions = sqlx_hashes(&app).await?.len();

    let logout = app
        .post_form_from("/logout", "", "https://evil.example")
        .await?;
    assert_eq!(logout.status(), StatusCode::FORBIDDEN);
    assert!(
        logout.body().contains("Request refused"),
        "{}",
        logout.body()
    );
    assert_eq!(
        sqlx_hashes(&app).await?.len(),
        sessions,
        "the session survived"
    );

    let login = app
        .signed_out()
        .post_form_from(
            "/login",
            "password=correct+horse+battery",
            "https://evil.example",
        )
        .await?;
    assert_eq!(login.status(), StatusCode::FORBIDDEN);
    assert!(login.headers().get(header::SET_COOKIE).is_none());

    let null = app.post_form_from("/logout", "", "null").await?;
    assert_eq!(null.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
async fn a_post_that_says_nothing_about_where_it_is_from_is_refused() -> TestResult {
    let app = app().await?;
    let bare = app
        .send(Request::post("/logout").body(Body::empty())?)
        .await?;
    assert_eq!(bare.status(), StatusCode::FORBIDDEN);
    let same = app
        .send(
            Request::post("/logout")
                .header("sec-fetch-site", "same-origin")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(same.status(), StatusCode::SEE_OTHER);
    Ok(())
}

#[tokio::test]
async fn a_session_that_ran_out_or_never_was_does_not_work() -> TestResult {
    let app = anonymous().await?;
    let token = "ab".repeat(32);
    let hash = blake3::hash(token.as_bytes());
    app.store
        .create_session(hash.as_bytes(), now_ms() - 10_000, now_ms() - 1)
        .await?;
    let expired = App {
        cookie: Some(format!("vigil_session={token}")),
        ..app.clone()
    };
    assert_eq!(expired.get("/").await?.status(), StatusCode::SEE_OTHER);

    for bad in [
        "vigil_session=",
        "vigil_session=zzzz",
        "vigil_session=ffffffff",
        "other=1",
    ] {
        let guest = App {
            cookie: Some(bad.to_owned()),
            ..app.clone()
        };
        assert_eq!(
            guest.get("/").await?.status(),
            StatusCode::SEE_OTHER,
            "{bad}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_session_in_use_is_renewed_but_not_on_every_click() -> TestResult {
    let app = anonymous().await?;
    let token = "cd".repeat(32);
    let hash = blake3::hash(token.as_bytes());
    let long_ago = now_ms() - 3 * 60 * 60 * 1000;
    app.store
        .create_session(hash.as_bytes(), long_ago, now_ms() + 1000)
        .await?;
    let browser = App {
        cookie: Some(format!("vigil_session={token}")),
        ..app.clone()
    };

    assert_eq!(browser.get("/").await?.status(), StatusCode::OK);
    let renewed = app
        .store
        .session(hash.as_bytes(), now_ms())
        .await?
        .ok_or("gone")?;
    assert!(
        renewed.last_seen_at > long_ago + 60 * 60 * 1000,
        "{renewed:?}"
    );
    assert!(
        renewed.expires_at > now_ms() + 29 * 24 * 60 * 60 * 1000,
        "{renewed:?}"
    );

    // A second click right away writes nothing.
    assert_eq!(browser.get("/").await?.status(), StatusCode::OK);
    let again = app
        .store
        .session(hash.as_bytes(), now_ms())
        .await?
        .ok_or("gone")?;
    assert_eq!(again, renewed);
    Ok(())
}

#[tokio::test]
async fn assets_and_the_health_check_need_no_session() -> TestResult {
    let app = anonymous().await?;
    assert_eq!(app.get("/assets/vigil.css").await?.status(), StatusCode::OK);
    assert_eq!(app.get("/healthz").await?.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn the_session_cookie_is_secure_over_https_and_scoped_to_the_base_path() -> TestResult {
    let app = anonymous_with(WebConfig::new(
        Auth::Password,
        Some("https://vigil.example.com/vigil"),
    ))
    .await?;
    // Every spelling of the home address needs the session: the one with a slash is only a
    // redirect to the one without.
    let slash = app.get("/vigil/").await?;
    assert_eq!(slash.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(location(&slash), "/vigil");
    let home = app.get("/vigil").await?;
    assert_eq!(home.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&home), "/vigil/login");

    let response = app
        .post_form_from(
            "/vigil/login",
            "password=correct+horse+battery&next=%2Fvigil%2Ftags",
            "https://vigil.example.com",
        )
        .await?;
    assert_eq!(location(&response), "/vigil/tags");
    let cookie = header_of(&response, "set-cookie");
    assert!(
        cookie.contains("Path=/vigil;") && cookie.ends_with("; Secure"),
        "{cookie}"
    );

    // `next` cannot leave the base path.
    let outside = app
        .post_form_from(
            "/vigil/login",
            "password=correct+horse+battery&next=%2Ftags",
            "https://vigil.example.com",
        )
        .await?;
    assert_eq!(location(&outside), "/vigil/");
    Ok(())
}

#[tokio::test]
async fn without_sign_in_every_page_is_open_and_login_goes_home() -> TestResult {
    let app = app_with(WebConfig::new(Auth::Disabled, None)).await?;
    assert_eq!(app.get("/").await?.status(), StatusCode::OK);
    let login = app.get("/login").await?;
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&login), "/");
    let post = app.post_form("/login", "password=whatever").await?;
    assert_eq!(location(&post), "/");
    assert!(post.headers().get(header::SET_COOKIE).is_none());
    Ok(())
}

#[tokio::test]
async fn without_sign_in_only_addresses_that_cannot_be_rebound_are_served() -> TestResult {
    let app = app_with(WebConfig::new(Auth::Disabled, None)).await?;
    let at = |host: &'static str| {
        let app = app.clone();
        async move {
            app.send(
                Request::get("/")
                    .header(header::HOST, host)
                    .body(Body::empty())?,
            )
            .await
        }
    };
    for ok in [
        "127.0.0.1:5800",
        "192.168.1.9:5800",
        "[::1]:5800",
        "localhost:5800",
    ] {
        assert_eq!(at(ok).await?.status(), StatusCode::OK, "{ok}");
    }
    let rebound = at("attacker.example:5800").await?;
    assert_eq!(rebound.status(), StatusCode::FORBIDDEN);
    assert!(
        rebound.body().contains("Address not allowed"),
        "{}",
        rebound.body()
    );
    // The health check is for orchestrators that call by service name.
    let health = app
        .send(
            Request::get("/healthz")
                .header(header::HOST, "vigil:5800")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(health.status(), StatusCode::OK);

    // Naming the address makes it fine.
    let named = app_with(WebConfig::new(Auth::Disabled, Some("http://nas.lan:5800"))).await?;
    let ok = named
        .send(
            Request::get("/")
                .header(header::HOST, "nas.lan:5800")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(ok.status(), StatusCode::OK);

    // And a post from another site is refused here too.
    let cross = app
        .post_form_from("/logout", "", "https://evil.example")
        .await?;
    assert_eq!(cross.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
async fn a_password_is_made_up_once_and_kept() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = vigil_store::Store::open(&dir.path().join("vigil.db")).await?;

    let first = ensure_admin_password(&store, None).await?;
    let PasswordSource::Generated(password) = first else {
        return Err(format!("expected a generated password, got {first:?}").into());
    };
    assert!(password.len() >= 20, "{password}");
    let stored = store
        .get_setting("admin_password_hash")
        .await?
        .ok_or("no hash")?;
    assert!(stored.starts_with("$argon2id$"), "{stored}");
    assert!(
        !stored.contains(&password),
        "the password itself is not stored"
    );

    assert_eq!(
        ensure_admin_password(&store, None).await?,
        PasswordSource::Stored
    );
    assert_eq!(
        store.get_setting("admin_password_hash").await?.as_deref(),
        Some(stored.as_str()),
        "it is not made up again"
    );
    // An empty variable is no variable.
    assert_eq!(
        ensure_admin_password(&store, Some("")).await?,
        PasswordSource::Stored
    );

    // The generated password works for signing in.
    let router = vigil_web::router(vigil_web::AppState::new(
        store.clone(),
        WebConfig::new(Auth::Password, None),
    )?);
    let app = App {
        _dir: std::sync::Arc::new(dir),
        router,
        store,
        cookie: None,
        base_path: String::new(),
        origin: "http://localhost".to_owned(),
    };
    let response = app
        .post_form("/login", &format!("password={password}"))
        .await?;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(response.headers().get(header::SET_COOKIE).is_some());
    Ok(())
}

#[tokio::test]
async fn the_environment_password_wins_and_is_rehashed_only_when_it_changes() -> TestResult {
    let app = anonymous().await?;
    // The test app already chose its password from the environment.
    let first = app
        .store
        .get_setting("admin_password_hash")
        .await?
        .ok_or("no hash")?;

    assert_eq!(
        ensure_admin_password(&app.store, Some(PASSWORD)).await?,
        PasswordSource::Environment
    );
    assert_eq!(
        app.store
            .get_setting("admin_password_hash")
            .await?
            .as_deref(),
        Some(first.as_str()),
        "the same password is not hashed again"
    );

    ensure_admin_password(&app.store, Some("a different one")).await?;
    let second = app
        .store
        .get_setting("admin_password_hash")
        .await?
        .ok_or("no hash")?;
    assert_ne!(first, second);
    let old = app
        .post_form("/login", "password=correct+horse+battery")
        .await?;
    assert_eq!(old.status(), StatusCode::UNAUTHORIZED);
    let new = app.post_form("/login", "password=a+different+one").await?;
    assert_eq!(new.status(), StatusCode::SEE_OTHER);
    Ok(())
}
