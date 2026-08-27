//! Resolves a user's effective [`Role`] — necessary supporting
//! infrastructure for `crate::policy`, not itself named in `docs/DOMAIN.md`
//! §7's port list, but required by it: core's `policy` module doc comment
//! is explicit that "core has no membership table to consult, so the caller
//! (which does) resolves 'what role does this user hold here' before
//! calling in." This port *is* that caller-side resolver.
//!
//! ## Area-scoped roles
//!
//! Areas are the top-level container (`docs/DOMAIN.md` §3) but roles were
//! originally only ever project-scoped, which left area-level actions
//! (`ViewArea`, `ManageArea`) with nowhere to hang except System Admin, and
//! made `CreateProject` chicken-and-egg — a project that does not exist yet
//! has no project role to authorize its own creation. The project owner's
//! fix: **Areas are a real membership scope too, and a Project inherits its
//! Area's role** when it carries no explicit role of its own.
//!
//! An explicit [`MembershipQuery::project_role`] always wins over an
//! inherited Area role — including when it is *lower* — because an explicit
//! grant is a deliberate statement, not a floor. See
//! [`MembershipQuery::effective_role`].

use async_trait::async_trait;

use anamnesis_core::policy::Role;
use anamnesis_core::{AreaId, ProjectId, UserId};

use crate::error::RepoError;

/// Resolves role membership: system-wide, per-area, and per-project.
#[async_trait]
pub trait MembershipQuery: Send + Sync {
    /// Whether `user` is a System Admin — implicitly every `ProjectAdmin`
    /// and `Member` permission everywhere (`docs/DOMAIN.md` §3).
    async fn is_system_admin(&self, user: &UserId) -> Result<bool, RepoError>;

    /// `user`'s role local to `area`, ignoring System Admin status — `None`
    /// if they hold no membership row on this area at all. This is the role
    /// [`Self::effective_role`] inherits down to a project in this area that
    /// carries no explicit project role of its own.
    async fn area_role(&self, user: &UserId, area: AreaId) -> Result<Option<Role>, RepoError>;

    /// `user`'s role local to `project`, ignoring System Admin status *and*
    /// ignoring any Area-level role — `None` if they hold no explicit
    /// membership row on this particular project.
    async fn project_role(
        &self,
        user: &UserId,
        project: ProjectId,
    ) -> Result<Option<Role>, RepoError>;

    /// `user`'s *effective* role with respect to `area` alone: whatever
    /// [`Self::area_role`] says, otherwise `SystemAdmin` if they hold that
    /// globally, otherwise `None`.
    ///
    /// This is what a purely area-scoped action (`ViewArea`, `ManageArea`)
    /// should be gated on, and also what `CreateProject` is gated on — a
    /// project that does not exist yet has no project role to resolve, only
    /// the Area's.
    async fn effective_area_role(
        &self,
        user: &UserId,
        area: AreaId,
    ) -> Result<Option<Role>, RepoError> {
        if let Some(role) = self.area_role(user, area).await? {
            return Ok(Some(role));
        }
        if self.is_system_admin(user).await? {
            return Ok(Some(Role::SystemAdmin));
        }
        Ok(None)
    }

    /// `user`'s *effective* role with respect to `project`, which lives in
    /// `area`: precedence is **explicit project role, then inherited Area
    /// role, then System Admin, then `None`.**
    ///
    /// An explicit [`Self::project_role`] wins outright the moment it is
    /// present — even when it is *lower* than what the Area would grant —
    /// because an explicit grant is a deliberate statement about this one
    /// project, not a floor under it. Only when no explicit project role
    /// exists does the Area's role (and, failing that, System Admin) take
    /// over, via [`Self::effective_area_role`].
    ///
    /// This is what every project-scoped use case should call — calling
    /// `project_role` directly would wrongly deny both a System Admin and
    /// an Area-level member who hold no membership row on this particular
    /// project.
    async fn effective_role(
        &self,
        user: &UserId,
        project: ProjectId,
        area: AreaId,
    ) -> Result<Option<Role>, RepoError> {
        if let Some(role) = self.project_role(user, project).await? {
            return Ok(Some(role));
        }
        self.effective_area_role(user, area).await
    }
}
