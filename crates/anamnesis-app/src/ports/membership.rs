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
//! ## Composition: strongest grant wins, by analogy to `chmod`
//!
//! [`MembershipQuery::area_role`], [`MembershipQuery::project_role`], and
//! System Admin status are three *independent* grants, not a most-specific-
//! wins override chain. [`MembershipQuery::effective_role`] takes the
//! **strongest** of the three (via [`Role`]'s ladder ordering, `Member <
//! ProjectAdmin < SystemAdmin`), and [`MembershipQuery::effective_area_role`]
//! takes the strongest of System Admin and the Area grant. A grant must
//! never *subtract* capability — adding someone to a project as a Member
//! must not demote an Area Admin on that one project, exactly as adding a
//! `chmod` bit never removes one already held. See
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

    /// `user`'s *effective* role with respect to `area` alone: the
    /// **strongest** of [`Self::area_role`] and `SystemAdmin` (if they hold
    /// that globally), or `None` if neither grant exists.
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
        let area_role = self.area_role(user, area).await?;
        let admin_role = self
            .is_system_admin(user)
            .await?
            .then_some(Role::SystemAdmin);
        // `Option<Role>`'s derived `Ord` puts `None` below every `Some`, and
        // orders `Some(a)` vs `Some(b)` by `Role`'s ladder — so `.max` is
        // exactly "the stronger of the two grants, or `None` if neither".
        Ok(area_role.max(admin_role))
    }

    /// `user`'s *effective* role with respect to `project`, which lives in
    /// `area`: the **strongest** of System Admin status, the Area grant, and
    /// the Project grant — never their most-specific override.
    ///
    /// Each of [`Self::project_role`], [`Self::area_role`], and System Admin
    /// status is an independent grant, by analogy to `chmod`: adding one
    /// must never subtract capability another already grants. In
    /// particular, an explicit [`Self::project_role`] that is *lower* than
    /// the user's Area grant does **not** demote them on that project — it
    /// only matters when it is the *strongest* of the three.
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
        let project_role = self.project_role(user, project).await?;
        let area_effective_role = self.effective_area_role(user, area).await?;
        Ok(project_role.max(area_effective_role))
    }
}
