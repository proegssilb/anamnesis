//! `WebError`: maps `anamnesis_app::AppError` (and the web layer's own
//! failure modes — a bad CSRF token, a broken login callback) to an HTTP
//! response. `AppError` lives in `anamnesis-app`, `IntoResponse` lives in
//! `axum`; neither is local to this crate, so this wrapper is what lets the
//! orphan rule be satisfied.

use std::error::Error as StdError;

use anamnesis_app::{AppError, IdentityError, RepoError};
use axum::extract::multipart::MultipartError;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use minijinja::{Environment, context};

/// Every way a request handler in this crate can fail.
#[derive(Debug)]
pub enum WebError {
    App(AppError),
    /// A mutating form's `csrf_token` did not match the session's.
    CsrfMismatch,
    /// The OIDC login round trip could not be completed (missing or
    /// mismatched pending-login state, or the identity provider rejected
    /// it).
    LoginFailed(IdentityError),
    /// A request could not even be parsed (e.g. a malformed id in the path,
    /// or a `status` field naming no known `ProjectStatus`).
    BadRequest(String),
    /// A request body exceeded the router-wide `ANAMNESIS_MAX_BODY_BYTES`
    /// ceiling. Distinct from [`WebError::BadRequest`] because "too big" and
    /// "malformed" are different problems with different fixes: one is
    /// answered by uploading a smaller file or raising the limit, the other
    /// by fixing the client.
    PayloadTooLarge(String),
    /// A `Range` request header named a byte range this attachment's size
    /// cannot satisfy at all (`crate::handlers::tasks::attachments::parse_single_range`).
    /// Carries the attachment's real size, for the `Content-Range: bytes
    /// */{size}` header a 416 response is expected to carry — most call
    /// sites build that response directly rather than going through
    /// [`WebError::into_response_with`], since the generic error page has
    /// nowhere to put a header; this variant exists mainly so
    /// [`WebError::status_and_message`] has a defined answer if one ever
    /// does.
    RangeNotSatisfiable(u64),
    /// A template failed to render. Always a bug (a missing context
    /// variable, a broken template), never something a request caused — but
    /// it still has to become *some* response rather than a panic.
    Template(String),
}

impl From<AppError> for WebError {
    fn from(err: AppError) -> Self {
        WebError::App(err)
    }
}

/// A bare port failure (e.g. from `crate::handlers::access`, which calls
/// `MembershipQuery` directly rather than through a use case) is exactly an
/// `AppError::Repo` as far as this crate's error handling is concerned.
impl From<RepoError> for WebError {
    fn from(err: RepoError) -> Self {
        WebError::App(AppError::Repo(err))
    }
}

/// Maps a domain [`AppError`] (from `anamnesis-app`, shared by every
/// non-web caller too) to the HTTP status and user-facing message it
/// becomes here. Split out of [`WebError::status_and_message`] because this
/// half is really a different job wearing a `WebError` costume: it is pure
/// `AppError` → HTTP translation, with no knowledge of the web layer's own
/// failure modes (CSRF, login, bad requests, template rendering) that the
/// rest of that match handles.
fn app_status_and_message(err: &AppError) -> (StatusCode, String) {
    match err {
        AppError::NotFound => (StatusCode::NOT_FOUND, "That was not found.".to_string()),
        AppError::Forbidden => (
            StatusCode::FORBIDDEN,
            "You do not have access to do that.".to_string(),
        ),
        AppError::Rule(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()),
        AppError::Invalid(message) => (StatusCode::UNPROCESSABLE_ENTITY, message.clone()),
        AppError::Conflict => (
            StatusCode::CONFLICT,
            "That was changed by someone else in the meantime — reload and try again.".to_string(),
        ),
        AppError::ActiveProjectLimitExceeded => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "The active project limit has been reached.".to_string(),
        ),
        AppError::WipLimitExceeded => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "That column is already at its work-in-progress limit.".to_string(),
        ),
        AppError::LastSystemAdmin => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "That is the last System Admin — grant someone else System Admin first.".to_string(),
        ),
        AppError::Repo(e) => {
            if let Some(message) = payload_too_large_message(e) {
                return (StatusCode::PAYLOAD_TOO_LARGE, message.to_string());
            }
            tracing::error!(error = %e, "repository error");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Something went wrong on our end.".to_string(),
            )
        }
    }
}

/// Recovers a `MultipartError` from deep inside a `RepoError`'s cause
/// chain, if `.status() == PAYLOAD_TOO_LARGE` names one.
///
/// Streaming moved where a multipart read actually happens: it used to be
/// this crate's own `field.bytes()` call, so a body-limit overrun surfaced
/// right here as an ordinary `MultipartError`. Now the read happens inside
/// `BlobStore::put`, two crates away — the error arrives instead as
/// `io::Error` (wrapping the original `MultipartError`, via
/// `io::Error::other`) boxed inside a `RepoError`. Left unhandled, that
/// would silently regress every over-limit upload from 413 to 500. This
/// walks `RepoError`'s `.source()` chain to find it.
fn payload_too_large_message(err: &RepoError) -> Option<&'static str> {
    let mut cause: Option<&(dyn StdError + 'static)> = err.source();
    while let Some(e) = cause {
        if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
            // `io::Error::source()` delegates to *its own* custom payload's
            // source rather than returning the payload itself, so the
            // `MultipartError` has to be recovered via `get_ref()`, not by
            // continuing the `.source()` walk through this node.
            let multipart_err = io_err
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<MultipartError>());
            if let Some(multipart_err) = multipart_err
                && multipart_err.status() == StatusCode::PAYLOAD_TOO_LARGE
            {
                return Some("that upload is larger than this server accepts");
            }
        }
        cause = e.source();
    }
    None
}

impl WebError {
    fn status_and_message(&self) -> (StatusCode, String) {
        match self {
            WebError::App(err) => app_status_and_message(err),
            WebError::CsrfMismatch => (
                StatusCode::FORBIDDEN,
                "That form's security token was missing or stale. Please try again.".to_string(),
            ),
            WebError::LoginFailed(e) => {
                tracing::error!(error = %e, "login failed");
                (
                    StatusCode::BAD_REQUEST,
                    "Login could not be completed.".to_string(),
                )
            }
            WebError::BadRequest(message) => (StatusCode::BAD_REQUEST, message.clone()),
            WebError::PayloadTooLarge(message) => (StatusCode::PAYLOAD_TOO_LARGE, message.clone()),
            WebError::RangeNotSatisfiable(_) => (
                StatusCode::RANGE_NOT_SATISFIABLE,
                "that range is not satisfiable".to_string(),
            ),
            WebError::Template(message) => {
                tracing::error!(error = %message, "template render failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Something went wrong on our end.".to_string(),
                )
            }
        }
    }

    /// Wraps a MiniJinja error (template lookup or render failure) as a
    /// [`WebError::Template`], keeping its `Display` message.
    pub fn template(err: minijinja::Error) -> Self {
        WebError::Template(err.to_string())
    }

    /// Renders this error as a standalone `error.html` page.
    pub fn into_response_with(self, templates: &Environment<'static>) -> Response {
        let (status, message) = self.status_and_message();
        let body = templates
            .get_template("error.html")
            .and_then(|t| t.render(context! { status => status.as_u16(), message => message }))
            .unwrap_or_else(|_| message.clone());
        (status, Html(body)).into_response()
    }
}

/// A minimal `IntoResponse` for contexts with no template environment handy
/// (falls back to a plain-text body). Handlers that have `AppState` should
/// prefer [`WebError::into_response_with`] so the error still looks like
/// the rest of the app.
impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let (status, message) = self.status_and_message();
        (status, message).into_response()
    }
}
