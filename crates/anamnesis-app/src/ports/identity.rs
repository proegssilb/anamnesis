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

/// The claim values a completed login resolves: a stable identity anchor, a
/// human-readable label for it, and the groups the provider asserted. Which
/// OIDC claim each comes from is a deployment-time choice — different
/// identity providers populate the standard claims differently, and some
/// deployments need a non-standard claim — so no field is assumed to be any
/// particular claim name by anything downstream of
/// [`IdentityProvider::complete_login`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedIdentity {
    /// The stable identity anchor threaded through sessions, membership, and
    /// bootstrap-admin matching.
    pub user_id: UserId,
    /// The label shown to and about this user in the UI.
    pub display_name: String,
    /// The groups the provider asserted, from the claim named by
    /// `ANAMNESIS_OIDC_GROUPS_CLAIM` — empty when that is unconfigured,
    /// which is the default. Recorded at login through
    /// [`crate::ports::GroupMembershipRepository::replace_user_groups`];
    /// membership alone grants nothing until a System Admin maps a group to
    /// a role.
    pub groups: Vec<String>,
}

/// Authenticates a user against an external OIDC provider. Anamnesis never
/// sees or stores a password.
#[async_trait]
pub trait IdentityProvider: Send + Sync {
    /// Begins a login: builds the authorization URL plus the PKCE/nonce/CSRF
    /// state the caller must retain to validate the eventual callback.
    async fn begin_login(&self) -> Result<LoginRedirect, IdentityError>;

    /// Completes a login: exchanges the authorization code for tokens,
    /// validates the ID token (signature, issuer, audience, nonce), and
    /// resolves the configured user-id and display-name claims.
    async fn complete_login(
        &self,
        callback: LoginCallback,
    ) -> Result<AuthenticatedIdentity, IdentityError>;
}
