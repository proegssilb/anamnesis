//! Authentication ports: not part of `docs/DOMAIN.md`'s domain model at all
//! (no entity there represents "an OIDC round trip"), but genuinely shared
//! infrastructure the web shell needs regardless of which domain model sits
//! behind it — carried over unchanged from the disposable kanban scaffold's
//! `legacy::ports` when Phase F1 retired everything else in that module.

use async_trait::async_trait;

use anamnesis_core::UserId;

use crate::error::IdentityError;

/// The information needed to send the user to the identity provider to begin
/// an OAuth2 Authorization Code + PKCE login, plus what must be retained
/// (typically in the session) to validate the callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginRedirect {
    /// The URL to redirect the user's browser to.
    pub authorize_url: String,
    /// Opaque CSRF state, echoed back by the provider on the callback.
    pub csrf_state: String,
    /// The PKCE code verifier, needed to complete the exchange.
    pub pkce_verifier: String,
    /// The nonce embedded in the authorization request, checked against the
    /// returned ID token's `nonce` claim.
    pub nonce: String,
}

/// The provider's callback, plus the state a [`LoginRedirect`] asked the
/// caller to retain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginCallback {
    /// The authorization code returned by the provider.
    pub code: String,
    /// The `state` query parameter returned by the provider, checked against
    /// [`LoginRedirect::csrf_state`].
    pub state: String,
    /// Echoed back from [`LoginRedirect::csrf_state`] by the caller.
    pub expected_state: String,
    /// Echoed back from [`LoginRedirect::pkce_verifier`] by the caller.
    pub pkce_verifier: String,
    /// Echoed back from [`LoginRedirect::nonce`] by the caller.
    pub expected_nonce: String,
}

/// Authenticates a user against an external OIDC provider. Anamnesis never
/// sees or stores a password; identity is the token's `sub` claim.
#[async_trait]
pub trait IdentityProvider: Send + Sync {
    /// Begins a login: builds the authorization URL plus the PKCE/nonce/CSRF
    /// state the caller must retain to validate the eventual callback.
    async fn begin_login(&self) -> Result<LoginRedirect, IdentityError>;

    /// Completes a login: exchanges the authorization code for tokens,
    /// validates the ID token (signature, issuer, audience, nonce), and
    /// returns the authenticated user's id (the `sub` claim).
    async fn complete_login(&self, callback: LoginCallback) -> Result<UserId, IdentityError>;
}
