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

use anamnesis_adapters::{FsBlobStore, SqlStore, SystemClock, TzTimezoneResolver, UuidIdGen};
use anamnesis_web::session::SessionData;
use anamnesis_web::state::AppState;
use anamnesis_web::{bootstrap, routes, session, templates};

/// 64 bytes exactly — the floor `ANAMNESIS_SESSION_SECRET` and
/// `axum_extra`'s `Key` both enforce.
pub const TEST_SESSION_SECRET: &str =
    "test-session-secret-0123456789-abcdefghijklmnopqrstuvwxyz-0123456789-ok!!";

/// A fixed CSRF token handed to the dev-bypass user in tests, so tests don't
/// need to scrape it out of rendered HTML to build a valid form submission.
pub const DEV_CSRF_TOKEN: &str = "test-dev-csrf-token";

/// The dev-bypass user's id — `anamnesis_web::auth::DEV_USER_ID`, duplicated
/// here as a plain string constant so tests need not depend on that private
/// module path.
pub const DEV_USER_ID: &str = "dev-user";

pub struct TestApp {
    router: Router,
    pub key: Key,
    pub store: Arc<SqlStore>,
    pub blob_root: std::path::PathBuf,
    _dir: tempfile::TempDir,
    _blob_dir: tempfile::TempDir,
}

impl TestApp {
    /// Builds an app talking to a fresh temp-file SQLite database, bootstrapped
    /// exactly as `main.rs` would (`DEV_USER_ID` granted System Admin, default
    /// board columns seeded). `dev_auth_bypass` mirrors
    /// `ANAMNESIS_DEV_AUTH_BYPASS`.
    pub async fn new(dev_auth_bypass: bool) -> Self {
        Self::with_bootstrap_admin(dev_auth_bypass, DEV_USER_ID).await
    }

    /// As [`TestApp::new`], but bootstraps `bootstrap_admin` as System Admin
    /// instead of the dev-bypass user — for tests that need a distinct,
    /// real-OIDC-style admin identity.
    pub async fn with_bootstrap_admin(dev_auth_bypass: bool, bootstrap_admin: &str) -> Self {
        assert!(
            TEST_SESSION_SECRET.len() >= 64,
            "test session secret must meet the same floor as production"
        );

        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("test.db");
        let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
        let store = SqlStore::connect(&db_url)
            .await
            .expect("connect to temp SQLite database");
        let id_gen = UuidIdGen;
        bootstrap::run(&store, &id_gen, bootstrap_admin, "UTC")
            .await
            .expect("bootstrap a fresh test database");
        let store = Arc::new(store);

        let key = Key::from(TEST_SESSION_SECRET.as_bytes());

        let blob_dir = tempfile::tempdir().expect("create temp blob dir");
        let blob_root = blob_dir.path().to_path_buf();
        let blobs = FsBlobStore::new(&blob_root)
            .await
            .expect("create temp blob store");

        let state = AppState {
            areas: store.clone(),
            projects: store.clone(),
            tasks: store.clone(),
            relationships: store.clone(),
            tangles: store.clone(),
            comments: store.clone(),
            attachments: store.clone(),
            blobs: Arc::new(blobs),
            board: store.clone(),
            search: store.clone(),
            search_index: store.clone(),
            membership: store.clone(),
            membership_write: store.clone(),
            timezone: Arc::new(TzTimezoneResolver::new()),
            clock: Arc::new(SystemClock),
            id_gen: Arc::new(id_gen),
            identity: None,
            templates: Arc::new(templates::build_environment()),
            cookie_key: key.clone(),
            dev_auth_bypass,
            dev_csrf_token: DEV_CSRF_TOKEN.to_string(),
            secure_cookies: false,
            settings: store.clone(),
            timezone_name: "UTC".to_string(),
        };

        let router = routes::build_router(state);
        Self {
            router,
            key,
            store,
            blob_root,
            _dir: dir,
            _blob_dir: blob_dir,
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
        self.get_maybe_hx(path, cookie, false).await
    }

    /// As [`TestApp::get`], but with `HX-Request: true` set — what a real
    /// htmx-driven navigation sends, and what a handler branches on to
    /// return a fragment instead of a full page (`docs/DOMAIN.md` §8).
    pub async fn get_hx(&self, path: &str, cookie: Option<&str>) -> Response<Body> {
        self.get_maybe_hx(path, cookie, true).await
    }

    async fn get_maybe_hx(&self, path: &str, cookie: Option<&str>, hx: bool) -> Response<Body> {
        let mut builder = Request::builder().method("GET").uri(path);
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        if hx {
            builder = builder.header("HX-Request", "true");
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
        self.post_form_maybe_hx(path, form, cookie, false).await
    }

    /// As [`TestApp::post_form`], but with `HX-Request: true` set — the same
    /// request an htmx-driven form submit (or `htmx.ajax`, as
    /// `static/app.js`'s drag handler uses) sends.
    pub async fn post_form_hx(
        &self,
        path: &str,
        form: &[(&str, &str)],
        cookie: Option<&str>,
    ) -> Response<Body> {
        self.post_form_maybe_hx(path, form, cookie, true).await
    }

    /// Posts a `multipart/form-data` request — what a real
    /// `<input type="file">` form submits, and what
    /// `crate::handlers::tasks::add_file_attachment_handler`'s
    /// `axum::extract::Multipart` extractor needs. `text_fields` become
    /// plain form parts (e.g. `csrf_token`); `file_field` is
    /// `(field_name, filename, bytes, content_type)` for the one file part.
    pub async fn post_multipart(
        &self,
        path: &str,
        text_fields: &[(&str, &str)],
        file_field: (&str, &str, &[u8], &str),
        cookie: Option<&str>,
    ) -> Response<Body> {
        let boundary = "----anamnesisTestBoundary7331";
        let mut body = Vec::new();
        for (name, value) in text_fields {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
            );
            body.extend_from_slice(value.as_bytes());
            body.extend_from_slice(b"\r\n");
        }
        let (field_name, filename, bytes, content_type) = file_field;
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{field_name}\"; filename=\"{filename}\"\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        let mut builder = Request::builder().method("POST").uri(path).header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        );
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        let request = builder.body(Body::from(body)).unwrap();
        self.router.clone().oneshot(request).await.unwrap()
    }

    async fn post_form_maybe_hx(
        &self,
        path: &str,
        form: &[(&str, &str)],
        cookie: Option<&str>,
        hx: bool,
    ) -> Response<Body> {
        let body = serde_urlencoded::to_string(form).unwrap();
        let mut builder = Request::builder()
            .method("POST")
            .uri(path)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        if hx {
            builder = builder.header("HX-Request", "true");
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

/// Creates an area, a project in it, and returns the project's path — the
/// shared setup every task-picker test needs before it can create tasks.
pub async fn new_project(app: &TestApp, cookie: Option<&str>) -> String {
    let area_path = location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", DEV_CSRF_TOKEN),
                ("title", "Homesteading"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    location_of(
        &app.post_form(
            &format!("{area_path}/projects"),
            &[
                ("csrf_token", DEV_CSRF_TOKEN),
                ("title", "Renovation"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string()
}

/// Creates a task under `project_path` and returns its path.
pub async fn new_task(
    app: &TestApp,
    project_path: &str,
    title: &str,
    cookie: Option<&str>,
) -> String {
    location_of(
        &app.post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", DEV_CSRF_TOKEN),
                ("title", title),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string()
}

/// Bootstraps a fresh admin-owned area/project/task under real OIDC-style
/// sessions (dev-auth-bypass off) and hands back the running app, the task's
/// path, and a signed cookie for a "stranger" identity that has never been
/// granted anything on it — the shared setup both the parent-picker's and
/// the relationship-picker's "without a view grant" tests need. Bypass has
/// to be off here — under it, every cookie resolves to the same dev user, so
/// a *distinct* ungranted identity needs real sessions (matching
/// `f3_admin_routes.rs`'s own "ungranted user" tests).
pub async fn setup_task_as_admin() -> (TestApp, String, String) {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", "admin-token"),
                ("title", "Homesteading"),
                ("description", ""),
            ],
            Some(&admin_cookie),
        )
        .await,
    )
    .to_string();
    let project_path = location_of(
        &app.post_form(
            &format!("{area_path}/projects"),
            &[
                ("csrf_token", "admin-token"),
                ("title", "Renovation"),
                ("description", ""),
            ],
            Some(&admin_cookie),
        )
        .await,
    )
    .to_string();
    let task_path = location_of(
        &app.post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", "admin-token"),
                ("title", "Regrout the shower"),
                ("description", ""),
            ],
            Some(&admin_cookie),
        )
        .await,
    )
    .to_string();

    let stranger_cookie = app.login_cookie_header("stranger", "stranger-token");
    (app, task_path, stranger_cookie)
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
