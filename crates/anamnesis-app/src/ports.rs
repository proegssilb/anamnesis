//! Port traits: what the use cases need from the world, declared here and
//! implemented by adapters (Phase 3) and the web shell (Phase 4). Nothing in
//! this module names a concrete database, HTTP, or OIDC crate.

use async_trait::async_trait;
use serde::Serialize;

use anamnesis_core::{BoardId, Timestamp, Title, UserId};

use crate::error::{IdentityError, RepoError};

/// The full aggregate this port trades in.
pub use anamnesis_core::legacy::Board;

/// A lightweight summary of a board, for listing without loading every
/// column and card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoardSummary {
    pub id: BoardId,
    pub title: Title,
}

/// Loads, saves, lists, and deletes `Board` aggregates.
///
/// `save` writes the whole aggregate in one transaction (delete-and-reinsert
/// columns and cards) — last-write-wins, an accepted tradeoff documented in
/// `ARCHITECTURE.md`.
#[async_trait]
pub trait BoardRepository: Send + Sync {
    async fn load(&self, id: BoardId) -> Result<Option<Board>, RepoError>;
    async fn save(&self, board: &Board) -> Result<(), RepoError>;
    async fn list_for_owner(&self, owner: &UserId) -> Result<Vec<BoardSummary>, RepoError>;
    async fn delete(&self, id: BoardId) -> Result<(), RepoError>;
}

/// Supplies "now" as a parameter to use cases that need it. The core never
/// reads a clock; this is where the shell's clock enters the system.
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

/// Supplies freshly minted ids to use cases that need them. The core never
/// generates one; this is where the shell's randomness enters the system.
pub trait IdGen: Send + Sync {
    fn next(&self) -> uuid::Uuid;
}

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
