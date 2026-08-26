//! The session: a signed, `HttpOnly`, `SameSite=Lax` cookie holding the
//! user id, display name, and CSRF token — see `docs/ARCHITECTURE.md`. A
//! signed cookie (not encrypted) is enough here: nothing in it is secret,
//! but the server must be the only party able to produce a value that
//! verifies, which is exactly what an HMAC signature buys.

use anamnesis_core::UserId;
use axum_extra::extract::cookie::{Cookie, SameSite};
use serde::{Deserialize, Serialize};

pub const SESSION_COOKIE_NAME: &str = "anamnesis_session";

/// A short-lived cookie holding the state a `GET /login` handed out, needed
/// to validate the eventual `GET /auth/callback`. Separate from
/// [`SessionData`] because it exists only for the few seconds of the OIDC
/// round trip and never becomes an authenticated session.
pub const PENDING_LOGIN_COOKIE_NAME: &str = "anamnesis_pending_login";

/// What the session cookie carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionData {
    pub user_id: UserId,
    pub display_name: String,
    pub csrf_token: String,
}

/// The state retained between `GET /login` and `GET /auth/callback`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingLogin {
    pub csrf_state: String,
    pub pkce_verifier: String,
    pub nonce: String,
}

/// Builds the `Set-Cookie` value for a freshly established session.
/// `secure` should be `true` whenever the app is served over HTTPS.
pub fn session_cookie(data: &SessionData, secure: bool) -> Cookie<'static> {
    let value = serde_json::to_string(data).expect("SessionData always serializes");
    build_cookie(SESSION_COOKIE_NAME, value, secure)
}

/// Parses a session cookie's value back into [`SessionData`]. `None` for
/// any cookie that is missing, malformed, or tampered with (a signed jar
/// already strips a cookie whose signature does not verify, so reaching
/// this function at all means the signature was valid — but the payload
/// shape is still checked here).
pub fn parse_session(raw: &str) -> Option<SessionData> {
    serde_json::from_str(raw).ok()
}

/// Builds the `Set-Cookie` value for the short-lived pending-login state.
pub fn pending_login_cookie(data: &PendingLogin, secure: bool) -> Cookie<'static> {
    let value = serde_json::to_string(data).expect("PendingLogin always serializes");
    build_cookie(PENDING_LOGIN_COOKIE_NAME, value, secure)
}

pub fn parse_pending_login(raw: &str) -> Option<PendingLogin> {
    serde_json::from_str(raw).ok()
}

/// A cookie that immediately expires `name`, used to clear a session or
/// pending-login cookie on logout / after a completed callback.
pub fn removal_cookie(name: &'static str) -> Cookie<'static> {
    let mut cookie = Cookie::from(name);
    cookie.set_path("/");
    cookie.make_removal();
    cookie
}

fn build_cookie(name: &'static str, value: String, secure: bool) -> Cookie<'static> {
    Cookie::build((name, value))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .path("/")
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_round_trips_through_json() {
        let data = SessionData {
            user_id: UserId::new("alice"),
            display_name: "Alice".to_string(),
            csrf_token: "tok123".to_string(),
        };
        let cookie = session_cookie(&data, false);
        let parsed = parse_session(cookie.value()).expect("valid session parses");
        assert_eq!(parsed, data);
    }

    #[test]
    fn session_cookie_is_http_only_lax_and_path_root() {
        let data = SessionData {
            user_id: UserId::new("alice"),
            display_name: "Alice".to_string(),
            csrf_token: "tok".to_string(),
        };
        let cookie = session_cookie(&data, true);
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
        assert_eq!(cookie.secure(), Some(true));
        assert_eq!(cookie.path(), Some("/"));
    }

    #[test]
    fn garbage_does_not_parse_as_a_session() {
        assert_eq!(parse_session("not json"), None);
    }

    #[test]
    fn pending_login_round_trips() {
        let data = PendingLogin {
            csrf_state: "state".to_string(),
            pkce_verifier: "verifier".to_string(),
            nonce: "nonce".to_string(),
        };
        let cookie = pending_login_cookie(&data, false);
        let parsed = parse_pending_login(cookie.value()).expect("valid pending login parses");
        assert_eq!(parsed, data);
    }
}
