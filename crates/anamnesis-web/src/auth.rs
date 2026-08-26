//! [`CurrentUser`]: the extractor every protected handler takes. Resolves
//! either to the dev-auth-bypass fixed user or to whatever is in a valid,
//! signed session cookie; anything else rejects with a redirect to
//! `/login`, which is what turns "unauthenticated board access" into "a
//! redirect" for free at the extractor layer rather than something every
//! handler has to remember to check.

use anamnesis_core::UserId;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Key, SignedCookieJar};

use crate::session::{SESSION_COOKIE_NAME, parse_session};
use crate::state::AppState;

/// The fixed identity `ANAMNESIS_DEV_AUTH_BYPASS` logs every request in as.
pub const DEV_USER_ID: &str = "dev-user";
pub const DEV_DISPLAY_NAME: &str = "Dev User";

/// The authenticated user for the current request, plus the CSRF token
/// every mutating form on their behalf must echo back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentUser {
    pub user_id: UserId,
    pub display_name: String,
    pub csrf_token: String,
}

impl FromRequestParts<AppState> for CurrentUser {
    /// A full `Response` (not a status code) so the rejection can be an
    /// actual redirect to `/login` — exactly what an unauthenticated
    /// request should get, straight from the extractor.
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if state.dev_auth_bypass {
            return Ok(CurrentUser {
                user_id: UserId::new(DEV_USER_ID),
                display_name: DEV_DISPLAY_NAME.to_string(),
                csrf_token: state.dev_csrf_token.clone(),
            });
        }

        // Infallible: a `SignedCookieJar` always builds, it just contains no
        // cookies that failed verification (those are silently dropped).
        let jar = SignedCookieJar::<Key>::from_request_parts(parts, state)
            .await
            .unwrap_or_else(|infallible: std::convert::Infallible| match infallible {});

        let session = jar
            .get(SESSION_COOKIE_NAME)
            .and_then(|cookie| parse_session(cookie.value()));

        match session {
            Some(session) => Ok(CurrentUser {
                user_id: session.user_id,
                display_name: session.display_name,
                csrf_token: session.csrf_token,
            }),
            None => Err(Redirect::to("/login").into_response()),
        }
    }
}
