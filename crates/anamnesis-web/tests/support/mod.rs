//! Shared harness for the `web` integration tests: a real `axum::Router`
//! built from `anamnesis_web`'s own public wiring (`routes::build_router`),
//! backed by a temp-file SQLite database — the same code path `main.rs`
//! uses, minus a real socket. `tower::ServiceExt::oneshot` drives it.
//!
//! This module is compiled fresh into every `tests/*.rs` binary (Rust's
//! usual `tests/support/mod.rs` pattern), and no single test file exercises
//! every helper here — hence the blanket `dead_code` allow rather than one
//! per unused item.
#![allow(dead_code)]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use axum_extra::extract::cookie::{Cookie, Key, SignedCookieJar};
use http_body_util::BodyExt;
use tower::ServiceExt;

use anamnesis_adapters::{SqlBoardRepository, SystemClock, UuidIdGen};
use anamnesis_web::session::SessionData;
use anamnesis_web::state::AppState;
use anamnesis_web::{routes, session, templates};

/// 64 bytes exactly — the floor `ANAMNESIS_SESSION_SECRET` and
/// `axum_extra`'s `Key` both enforce.
pub const TEST_SESSION_SECRET: &str =
    "test-session-secret-0123456789-abcdefghijklmnopqrstuvwxyz-0123456789-ok!!";

/// A fixed CSRF token handed to the dev-bypass user in tests, so tests don't
/// need to scrape it out of rendered HTML to build a valid form submission.
pub const DEV_CSRF_TOKEN: &str = "test-dev-csrf-token";

pub struct TestApp {
    router: Router,
    pub key: Key,
    _dir: tempfile::TempDir,
}

impl TestApp {
    /// Builds an app talking to a fresh temp-file SQLite database.
    /// `dev_auth_bypass` mirrors `ANAMNESIS_DEV_AUTH_BYPASS`.
    pub async fn new(dev_auth_bypass: bool) -> Self {
        assert!(
            TEST_SESSION_SECRET.len() >= 64,
            "test session secret must meet the same floor as production"
        );

        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("test.db");
        let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
        let repo = SqlBoardRepository::connect(&db_url)
            .await
            .expect("connect to temp SQLite database");

        let key = Key::from(TEST_SESSION_SECRET.as_bytes());

        let state = AppState {
            repo: Arc::new(repo),
            clock: Arc::new(SystemClock),
            id_gen: Arc::new(UuidIdGen),
            identity: None,
            templates: Arc::new(templates::build_environment()),
            cookie_key: key.clone(),
            dev_auth_bypass,
            dev_csrf_token: DEV_CSRF_TOKEN.to_string(),
            secure_cookies: false,
        };

        let router = routes::build_router(state);
        Self {
            router,
            key,
            _dir: dir,
        }
    }

    /// Builds a `Cookie` header value carrying a validly signed session for
    /// `user_id`, as if that user had completed a real login. Used by tests
    /// that need two distinct authenticated identities, which dev-bypass
    /// (a single fixed user) cannot provide.
    pub fn login_cookie_header(&self, user_id: &str, csrf_token: &str) -> String {
        let session = SessionData {
            user_id: anamnesis_core::UserId::new(user_id),
            display_name: user_id.to_string(),
            csrf_token: csrf_token.to_string(),
        };
        let cookie = session::session_cookie(&session, false);
        signed_cookie_header(&self.key, cookie)
    }

    pub async fn get(&self, path: &str, cookie: Option<&str>) -> Response<Body> {
        let mut builder = Request::builder().method("GET").uri(path);
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        let request = builder.body(Body::empty()).unwrap();
        self.router.clone().oneshot(request).await.unwrap()
    }

    pub async fn post_form(
        &self,
        path: &str,
        form: &[(&str, &str)],
        cookie: Option<&str>,
    ) -> Response<Body> {
        let body = serde_urlencoded::to_string(form).unwrap();
        let mut builder = Request::builder()
            .method("POST")
            .uri(path)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        let request = builder.body(Body::from(body)).unwrap();
        self.router.clone().oneshot(request).await.unwrap()
    }
}

/// Signs `cookie` with `key` exactly as the app's own `SignedCookieJar`
/// would, and returns it as a `Cookie:` request-header-ready `name=value`
/// string.
fn signed_cookie_header(key: &Key, cookie: Cookie<'static>) -> String {
    let jar = SignedCookieJar::new(key.clone()).add(cookie);
    let response = axum::response::IntoResponse::into_response(jar);
    let set_cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("SignedCookieJar sets a Set-Cookie header")
        .to_str()
        .unwrap();
    set_cookie.split(';').next().unwrap().to_string()
}

pub fn location_of(response: &Response<Body>) -> &str {
    response
        .headers()
        .get(header::LOCATION)
        .expect("response has a Location header")
        .to_str()
        .unwrap()
}

pub async fn body_text(response: Response<Body>) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[allow(dead_code)]
pub fn assert_status(response: &Response<Body>, expected: StatusCode) {
    assert_eq!(
        response.status(),
        expected,
        "expected status {expected}, got {}",
        response.status()
    );
}
