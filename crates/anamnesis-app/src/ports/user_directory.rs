//! A best-effort local cache mapping a user id to the display name it last
//! presented at login. Not a users table — `docs/CONTEXT.md` and
//! `docs/ARCHITECTURE.md`'s "Authentication" section are both explicit that
//! there is no such thing and no plan for one: this carries no credential,
//! no role, and nothing here is ever consulted to make an authorization
//! decision. See `crate::use_cases::user_directory`'s module doc comment
//! for the two usability gaps this closes.

use std::collections::HashMap;

use async_trait::async_trait;

use anamnesis_core::UserId;

use crate::error::RepoError;

/// Resolves recorded display names. The write half is
/// [`UserDirectoryRepository`], split for the same reason every other
/// query/repository pair in this crate is: the read-only callers (rendering
/// a comment's author, an admin's members list) have no business holding
/// write capability, which only `crate::handlers::login`'s single call site
/// ever needs.
#[async_trait]
pub trait UserDirectoryQuery: Send + Sync {
    /// The recorded display name for every id in `users` that anamnesis has
    /// ever seen log in. An id with no entry — never logged in, or its only
    /// login predates this cache — is simply absent from the result;
    /// callers fall back to showing the raw id themselves.
    async fn display_names(&self, users: &[UserId]) -> Result<HashMap<UserId, String>, RepoError>;

    /// Every `(user, display_name)` this deployment has ever recorded.
    /// Purely a UI affordance backing the datalist behind a "grant a role"
    /// user-id input, exactly as `GroupMembershipQuery::list_known_groups`
    /// backs the group-name input next to it — never consult it to make an
    /// authorization decision.
    async fn list_known_users(&self) -> Result<Vec<(UserId, String)>, RepoError>;
}

/// The write half of [`UserDirectoryQuery`]: records what a login presented.
#[async_trait]
pub trait UserDirectoryRepository: Send + Sync {
    /// Records `user`'s current display name, upserting over any previously
    /// recorded value. Called once per login, unconditionally, exactly as
    /// `GroupMembershipRepository::replace_user_groups` is — this is the
    /// server writing down a fact about an identity the provider just
    /// authenticated, not an authorization decision.
    async fn remember(&self, user: &UserId, display_name: &str) -> Result<(), RepoError>;
}
