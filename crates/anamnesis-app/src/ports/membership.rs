//! Resolves a user's effective [`Role`] — necessary supporting
//! infrastructure for `crate::policy`, not itself named in `docs/DOMAIN.md`
//! §7's port list, but required by it: core's `policy` module doc comment
//! is explicit that "core has no membership table to consult, so the caller
//! (which does) resolves 'what role does this user hold here' before
//! calling in." This port *is* that caller-side resolver.

use async_trait::async_trait;

use anamnesis_core::policy::Role;
use anamnesis_core::{ProjectId, UserId};

use crate::error::RepoError;

/// Resolves role membership: system-wide, and per-project.
#[async_trait]
pub trait MembershipQuery: Send + Sync {
    /// Whether `user` is a System Admin — implicitly every `ProjectAdmin`
    /// and `Member` permission everywhere (`docs/DOMAIN.md` §3).
    async fn is_system_admin(&self, user: &UserId) -> Result<bool, RepoError>;

    /// `user`'s role local to `project`, ignoring System Admin status —
    /// `None` if they are not a member of this project at all.
    async fn project_role(
        &self,
        user: &UserId,
        project: ProjectId,
    ) -> Result<Option<Role>, RepoError>;

    /// `user`'s *effective* role with respect to `project`: `SystemAdmin` if
    /// they hold that globally, otherwise whatever [`Self::project_role`]
    /// returns. This is what every project-scoped use case should call —
    /// calling `project_role` directly would wrongly deny a System Admin who
    /// is not itself listed as a member of this particular project.
    async fn effective_role(
        &self,
        user: &UserId,
        project: ProjectId,
    ) -> Result<Option<Role>, RepoError> {
        if self.is_system_admin(user).await? {
            return Ok(Some(Role::SystemAdmin));
        }
        self.project_role(user, project).await
    }
}
