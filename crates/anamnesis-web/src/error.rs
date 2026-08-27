//! `WebError`: maps `anamnesis_app::AppError` (and the web layer's own
//! failure modes — a bad CSRF token, a broken login callback) to an HTTP
//! response. `AppError` lives in `anamnesis-app`, `IntoResponse` lives in
//! `axum`; neither is local to this crate, so this wrapper is what lets the
//! orphan rule be satisfied.

use anamnesis_app::{AppError, IdentityError, RepoError};
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

impl WebError {
    fn status_and_message(&self) -> (StatusCode, String) {
        match self {
            WebError::App(AppError::NotFound) => {
                (StatusCode::NOT_FOUND, "That was not found.".to_string())
            }
            WebError::App(AppError::Forbidden) => (
                StatusCode::FORBIDDEN,
                "You do not have access to do that.".to_string(),
            ),
            WebError::App(AppError::Rule(e)) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()),
            WebError::App(AppError::Invalid(message)) => {
                (StatusCode::UNPROCESSABLE_ENTITY, message.clone())
            }
            WebError::App(AppError::Conflict) => (
                StatusCode::CONFLICT,
                "That was changed by someone else in the meantime — reload and try again."
                    .to_string(),
            ),
            WebError::App(AppError::ActiveProjectLimitExceeded) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "The active project limit has been reached.".to_string(),
            ),
            WebError::App(AppError::WipLimitExceeded) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "That column is already at its work-in-progress limit.".to_string(),
            ),
            WebError::App(AppError::Repo(e)) => {
                tracing::error!(error = %e, "repository error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Something went wrong on our end.".to_string(),
                )
            }
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
