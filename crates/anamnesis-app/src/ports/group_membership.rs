//! The optional group dimension of role membership: a *second* source of
//! grants, alongside the per-user grants of [`crate::ports::membership`].
//!
//! A deployment that models access as groups in its identity provider
//! configures which OIDC claim carries them
//! (`ANAMNESIS_OIDC_GROUPS_CLAIM`); anamnesis records what that claim
//! asserted at login, and a System Admin maps a group name to a role exactly
//! as they would map a user. Unconfigured, every method here answers "no
//! grant" and nothing changes anywhere.
//!
//! ## The same composition rule, one dimension down
//!
//! [`GroupMembershipQuery::effective_area_role`] and
//! [`GroupMembershipQuery::effective_role`] are default methods composing
//! this port's three primitives exactly as
//! [`crate::ports::MembershipQuery`]'s namesakes compose its own: strongest
//! grant wins, `Member < ProjectAdmin < SystemAdmin`, never a
//! most-specific-wins override. They answer "what does this user's *group*
//! membership alone grant here".
//!
//! The two dimensions are then joined in one place — `crate::access` — which
//! is what every caller should actually use. Neither port composes the
//! other, so each stays independently testable and the final `.max()` lives
//! at a single site.
//!
//! ## Group membership is not itself a grant
//!
//! A recorded group (`replace_user_groups`) confers nothing. It only matters
//! where a System Admin has separately mapped that group to a role through
//! [`GroupMembershipRepository`]'s gated use cases in
//! [`crate::use_cases::group_membership`].

use async_trait::async_trait;

use anamnesis_core::policy::Role;
use anamnesis_core::{AreaId, ProjectId, UserId};

use crate::error::RepoError;

/// Resolves what a user's *group* membership grants: system-wide, per-area,
/// and per-project. The per-user equivalent is
/// [`crate::ports::MembershipQuery`].
#[async_trait]
pub trait GroupMembershipQuery: Send + Sync {
    /// Whether any group `user` belongs to is mapped to System Admin.
    async fn is_system_admin_via_group(&self, user: &UserId) -> Result<bool, RepoError>;

    /// The strongest role any of `user`'s groups holds on `area`, ignoring
    /// System-Admin group mappings — `None` if none of them is mapped here.
    async fn area_group_role(&self, user: &UserId, area: AreaId)
    -> Result<Option<Role>, RepoError>;

    /// The strongest role any of `user`'s groups holds on `project`,
    /// ignoring both System-Admin and Area-level group mappings.
    async fn project_group_role(
        &self,
        user: &UserId,
        project: ProjectId,
    ) -> Result<Option<Role>, RepoError>;

    /// `user`'s effective role on `area` **from group membership alone**:
    /// the stronger of [`Self::area_group_role`] and `SystemAdmin` (if any
    /// of their groups is mapped to it).
    async fn effective_area_role(
        &self,
        user: &UserId,
        area: AreaId,
    ) -> Result<Option<Role>, RepoError> {
        let area_role = self.area_group_role(user, area).await?;
        let admin_role = self
            .is_system_admin_via_group(user)
            .await?
            .then_some(Role::SystemAdmin);
        Ok(area_role.max(admin_role))
    }

    /// `user`'s effective role on `project` (which lives in `area`) **from
    /// group membership alone**: the strongest of the System-Admin, Area,
    /// and Project group mappings.
    async fn effective_role(
        &self,
        user: &UserId,
        project: ProjectId,
        area: AreaId,
    ) -> Result<Option<Role>, RepoError> {
        let project_role = self.project_group_role(user, project).await?;
        let area_effective_role = self.effective_area_role(user, area).await?;
        Ok(project_role.max(area_effective_role))
    }

    /// Every group currently mapped to System Admin — what the `/users`
    /// page lists, and the group-side counterpart of
    /// [`crate::ports::MembershipQuery::list_system_admins`].
    async fn list_admin_groups(&self) -> Result<Vec<String>, RepoError>;

    /// Every `(group, role)` pair mapped directly onto `area`.
    async fn list_area_groups(&self, area: AreaId) -> Result<Vec<(String, Role)>, RepoError>;

    /// Every `(group, role)` pair mapped directly onto `project`.
    async fn list_project_groups(
        &self,
        project: ProjectId,
    ) -> Result<Vec<(String, Role)>, RepoError>;

    /// Every distinct group name anamnesis has ever seen a user present,
    /// plus every group already mapped to a role.
    ///
    /// Purely a UI affordance: it backs the datalist behind the group-name
    /// inputs, so an admin picks a name that actually exists rather than
    /// typing one blind and silently creating a mapping that can never
    /// match. Never consult it to make an authorization decision.
    async fn list_known_groups(&self) -> Result<Vec<String>, RepoError>;
}

/// The write half of [`GroupMembershipQuery`], split for the same reason
/// [`crate::ports::MembershipRepository`] is: the read-only callers
/// (every permission check) have no business holding write capability.
#[async_trait]
pub trait GroupMembershipRepository: Send + Sync {
    /// Replaces `user`'s recorded group membership with exactly `groups`,
    /// atomically — called once per login with whatever the configured
    /// groups claim asserted.
    ///
    /// Wholesale replacement, not a merge: leaving a group in the identity
    /// provider must actually remove it here, and the row set must not grow
    /// without bound across logins. An empty `groups` clears them all.
    async fn replace_user_groups(&self, user: &UserId, groups: &[String]) -> Result<(), RepoError>;

    /// Maps `group` to System Admin — idempotent.
    async fn grant_admin_group(&self, group: &str) -> Result<(), RepoError>;
    /// Unmaps `group` from System Admin — idempotent.
    ///
    /// Deliberately carries no last-admin check, unlike
    /// [`crate::ports::MembershipRepository::revoke_system_admin`]: an
    /// admin-group mapping is not evidence that any user actually holds
    /// admin, so counting these rows cannot answer the question that check
    /// exists to ask. The per-user check over `system_admins` remains the
    /// real lockout guard.
    async fn revoke_admin_group(&self, group: &str) -> Result<(), RepoError>;

    /// Maps `group` to `role` on `area`, upserting over any existing
    /// mapping.
    async fn set_area_group_role(
        &self,
        group: &str,
        area: AreaId,
        role: Role,
    ) -> Result<(), RepoError>;
    /// Unmaps `group` from `area` entirely (idempotent).
    async fn revoke_area_group_role(&self, group: &str, area: AreaId) -> Result<(), RepoError>;

    /// Maps `group` to `role` on `project`, upserting over any existing
    /// mapping.
    async fn set_project_group_role(
        &self,
        group: &str,
        project: ProjectId,
        role: Role,
    ) -> Result<(), RepoError>;
    /// Unmaps `group` from `project` entirely (idempotent).
    async fn revoke_project_group_role(
        &self,
        group: &str,
        project: ProjectId,
    ) -> Result<(), RepoError>;
}
