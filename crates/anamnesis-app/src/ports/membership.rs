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

    /// Every user currently holding System Admin, in no particular order.
    ///
    /// Two callers need this: [`crate::use_cases::membership::
    /// revoke_system_admin`], to refuse revoking the very last one (an
    /// empty, or single-entry, result after the target is confirmed to be
    /// in it means "this is the last admin — refuse"), and a System-Admin-
    /// only "grant System Admin" UI, to show who already holds it.
    async fn list_system_admins(&self) -> Result<Vec<UserId>, RepoError>;

    /// Every `(user, role)` pair holding an *explicit* role directly on
    /// `area` — what an Area's "members" UI section lists. Deliberately not
    /// "every user with any effective access to this area": a System Admin
    /// with no area-level row of their own does not appear here, exactly as
    /// [`Self::area_role`] (which this is built to complement, not
    /// duplicate) would report `None` for them.
    async fn list_area_members(&self, area: AreaId) -> Result<Vec<(UserId, Role)>, RepoError>;

    /// Every `(user, role)` pair holding an *explicit* role directly on
    /// `project` — the [`Self::list_area_members`] sibling for a Project's
    /// "members" UI section.
    async fn list_project_members(
        &self,
        project: ProjectId,
    ) -> Result<Vec<(UserId, Role)>, RepoError>;
}

/// The write half of [`MembershipQuery`]: grants and revokes System Admin,
/// and Area/Project roles.
///
/// Kept as a **separate trait** rather than folded into [`MembershipQuery`],
/// for the same reason `docs/DOMAIN.md` §7 already splits `SearchQuery` from
/// `SearchIndex`: the two halves diverge at their call sites. Every
/// read-only use case (`view_area`, `view_project`, every permission check
/// in `crate::policy`) only ever needs [`MembershipQuery`]; only the small,
/// deliberately separate set of use cases in
/// [`crate::use_cases::membership`] ever needs to *write* a grant. Keeping
/// them apart means a future read-only caller (a report, a CLI listing) can
/// depend on [`MembershipQuery`] alone without pulling in write capability
/// it has no business holding — the same "narrowest port a caller actually
/// needs" discipline every other port split in this crate already follows.
///
/// Promotes what were, before this port existed, inherent seams on
/// `anamnesis_adapters::SqlStore` (`grant_system_admin`, `set_area_role`,
/// `set_project_role`) reached into directly by `anamnesis-web::bootstrap`
/// — the *only* place in the whole system that could ever grant a role,
/// which is precisely the gap this port closes: nothing above bootstrap
/// could ever grant a role to anyone else, so the bootstrap admin was
/// permanently the only user who could hold one.
#[async_trait]
pub trait MembershipRepository: Send + Sync {
    /// Grants `user` System Admin — idempotent (granting it twice is a
    /// no-op, not an error).
    async fn grant_system_admin(&self, user: &UserId) -> Result<(), RepoError>;
    /// Revokes `user`'s System Admin — idempotent (revoking from a user who
    /// does not hold it is a no-op, not an error). Callers must apply
    /// `crate::use_cases::membership::revoke_system_admin`'s last-admin
    /// check *before* calling this; this port method itself performs no
    /// such check, exactly as every other port in this crate leaves domain
    /// rules to the use-case layer above it.
    async fn revoke_system_admin(&self, user: &UserId) -> Result<(), RepoError>;
    /// Grants `user` `role` on `area`, upserting over any existing grant.
    async fn set_area_role(&self, user: &UserId, area: AreaId, role: Role)
    -> Result<(), RepoError>;
    /// Revokes `user`'s role on `area` entirely (idempotent).
    async fn revoke_area_role(&self, user: &UserId, area: AreaId) -> Result<(), RepoError>;
    /// Grants `user` `role` on `project`, upserting over any existing grant.
    async fn set_project_role(
        &self,
        user: &UserId,
        project: ProjectId,
        role: Role,
    ) -> Result<(), RepoError>;
    /// Revokes `user`'s role on `project` entirely (idempotent).
    async fn revoke_project_role(&self, user: &UserId, project: ProjectId)
    -> Result<(), RepoError>;
}
