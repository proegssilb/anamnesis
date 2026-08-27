//! [`AppState`]: everything a handler needs, cloned cheaply per request
//! (`Arc` all the way down). Built once at startup in `main.rs`; integration
//! tests build one directly against a temp-file `SqlStore` (see
//! `tests/support`).
//!
//! Every port field is a trait object (`Arc<dyn Trait>`), per
//! `docs/DOMAIN.md` §7's per-entity repository split — even though, in this
//! crate, every one of them happens to be backed by the same
//! `anamnesis_adapters::SqlStore` under the hood (constructed once in
//! `main.rs`/`tests/support` and coerced into each field). Keeping them as
//! separate ports rather than one concrete `Arc<SqlStore>` field is what
//! keeps `anamnesis-web` depending only on `anamnesis-app`'s ports, exactly
//! as every other crate boundary in this workspace does.

use std::sync::Arc;

use anamnesis_app::{
    AreaRepository, AttachmentRepository, BoardQuery, Clock, CommentRepository, IdGen,
    IdentityProvider, MembershipQuery, ProjectRepository, RelationshipRepository, SearchIndex,
    SearchQuery, TangleRepository, TaskRepository, TimezoneResolver,
};
use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;
use minijinja::Environment;

use crate::settings::AppSettings;

/// The application's shared state. Every field is `pub` on purpose: tests
/// build this struct directly (rather than through [`crate::config::Config`]
/// and real adapters) so they can inject a temp-file store and a fixed CSRF
/// token without going through environment variables or a live deployment
/// database.
#[derive(Clone)]
pub struct AppState {
    pub areas: Arc<dyn AreaRepository>,
    pub projects: Arc<dyn ProjectRepository>,
    pub tasks: Arc<dyn TaskRepository>,
    pub relationships: Arc<dyn RelationshipRepository>,
    pub tangles: Arc<dyn TangleRepository>,
    pub comments: Arc<dyn CommentRepository>,
    pub attachments: Arc<dyn AttachmentRepository>,
    pub board: Arc<dyn BoardQuery>,
    /// The read side of global search (`docs/DOMAIN.md` §8). `search_index`
    /// is the write side — kept as a separate field (rather than one
    /// combined trait object) because the two ports genuinely diverge at the
    /// call sites that use them: handlers rendering a result list only ever
    /// need `SearchQuery`, and handlers writing an area/project/task only
    /// ever need `SearchIndex`, exactly mirroring the port split itself.
    pub search: Arc<dyn SearchQuery>,
    pub search_index: Arc<dyn SearchIndex>,
    pub membership: Arc<dyn MembershipQuery>,
    pub timezone: Arc<dyn TimezoneResolver>,
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
    /// The knobs `docs/DOMAIN.md` §3 assigns to a `Settings` entity that no
    /// port in `anamnesis-app` reads or writes (`crate::settings`'s module
    /// doc comment explains the gap) — config-sourced for this phase.
    pub settings: AppSettings,
}

impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.cookie_key.clone()
    }
}
