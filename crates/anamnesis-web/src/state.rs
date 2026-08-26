//! [`AppState`]: everything a handler needs, cloned cheaply per request
//! (`Arc` all the way down). Built once at startup in `main.rs`; integration
//! tests build one directly against fake or temp-file-backed ports.

use std::sync::Arc;

use anamnesis_app::{BoardRepository, Clock, IdGen, IdentityProvider};
use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;
use minijinja::Environment;

/// The application's shared state. Every field is `pub` on purpose: tests
/// build this struct directly (rather than through [`crate::config::Config`]
/// and real adapters) so they can inject fakes and a fixed CSRF token
/// without going through environment variables or a live database.
#[derive(Clone)]
pub struct AppState {
    pub repo: Arc<dyn BoardRepository>,
    pub clock: Arc<dyn Clock>,
    pub id_gen: Arc<dyn IdGen>,
    /// `None` only when dev-auth-bypass is on and no OIDC provider was
    /// configured — the login/callback routes short-circuit before ever
    /// touching this, but the field stays optional rather than lying with a
    /// placeholder provider that nothing may call.
    pub identity: Option<Arc<dyn IdentityProvider>>,
    pub templates: Arc<Environment<'static>>,
    /// Signs and verifies the session and pending-login cookies. Derived
    /// from `ANAMNESIS_SESSION_SECRET`, which [`crate::config::Config`]
    /// already validates as at least 64 bytes — the same floor
    /// `axum_extra::extract::cookie::Key` itself enforces, so the two checks
    /// reinforce rather than duplicate each other.
    pub cookie_key: Key,
    pub dev_auth_bypass: bool,
    /// The fixed CSRF token handed to the dev-bypass user. Generated once at
    /// startup (or once per test `AppState`) rather than hard-coded, so it
    /// is still an unguessable value in memory even though bypass mode's
    /// whole point is skipping real authentication.
    pub dev_csrf_token: String,
    /// Whether the session cookie gets the `Secure` attribute — true when
    /// `ANAMNESIS_BASE_URL` is `https://`.
    pub secure_cookies: bool,
}

impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.cookie_key.clone()
    }
}
