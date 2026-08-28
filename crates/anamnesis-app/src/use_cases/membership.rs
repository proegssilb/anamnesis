//! Granting and revoking roles — the write half of `crate::ports::
//! MembershipQuery`, which was read-only, leaving the bootstrap admin
//! (`anamnesis-web::bootstrap`, reaching directly into
//! `anamnesis_adapters::SqlStore`'s inherent seams) as the only user who
//! could ever hold a role. Nothing in the use-case layer could dig anyone
//! else out of that hole: `docs/DOMAIN.md`'s multi-user model was complete
//! on paper and operationally unreachable. This module is what makes it
//! reachable.
//!
//! ## No privilege escalation
//!
//! Every function here is gated through `crate::policy` exactly like any
//! other use case, but granting a role carries one hazard ordinary
//! view/edit actions do not: the *content being written* can itself grant
//! capability. Two rules close that off, stated here explicitly because a
//! mistake in either is a real vulnerability, not a UX bug:
//!
//! - **System Admin can only ever be granted through
//!   [`grant_system_admin`]**, itself gated on `Action::ManageUsers`
//!   (System Admin only). [`grant_area_role`] and [`grant_project_role`]
//!   both reject `role == Role::SystemAdmin` outright (`AppError::
//!   Forbidden`) before ever reaching the repository — a Project Admin (who
//!   legitimately passes `Action::ManageArea`/`Action::
//!   ManageProjectMembership`) must never be able to write `"system_admin"`
//!   into an area/project membership row and reach system-wide authority
//!   through the back door. Nothing downstream would even *notice*: a
//!   project-scoped `"system_admin"` row only ever strengthens that one
//!   project's *effective* role to what a Project Admin already gets
//!   (`can_manage_project` accepts both), so the escalation is invisible
//!   unless it is refused right here, at the write.
//! - **A grant is checked against the actor's role *on that specific
//!   scope*, resolved by the caller exactly as every other use case
//!   resolves it** (`crate::ports::MembershipQuery::effective_area_role`/
//!   `effective_role`, e.g. via `anamnesis-web::handlers::access`) — so a
//!   Project Admin of one Area can never grant a role on a *different* Area
//!   they do not administer: `actor_role` for that other Area resolves to
//!   `None` (or a bare `Member` row, if they happen to hold one there too)
//!   and `is_allowed` refuses it, the exact same structural guarantee every
//!   other Area/Project-scoped action in this crate already relies on
//!   (`crate::policy`'s module doc comment).
//!
//! **Self-escalation is refused by those same two rules, not a separate
//! check.** Nothing here special-cases "is the target the same as the
//! actor": escalating your own role requires either passing
//! `Role::SystemAdmin` through an Area/Project grant (rejected outright,
//! regardless of target) or already holding admin authority over the exact
//! scope you are trying to strengthen (which is not an escalation at all —
//! it is that scope's admin doing ordinary membership work on themselves,
//! same as on anyone else).
//!
//! ## The last System Admin
//!
//! [`revoke_system_admin`] refuses to revoke System Admin from the last
//! user who holds it (`AppError::LastSystemAdmin`). Locking every admin out
//! of a self-hosted deployment is unrecoverable without direct database
//! access, so refusing is the sane default — an operator who genuinely
//! wants to remove the last admin (decommissioning, migrating to a
//! different bootstrap subject) still has the database itself available;
//! the application layer simply never does it *by accident*. In practice
//! this reduces to refusing *self*-revocation specifically: only a System
//! Admin can ever call this (`Action::ManageUsers`), so if there is only
//! one, they are the only person who could ever reach the "revoke the last
//! admin" call at all.

use anamnesis_core::policy::Role;
use anamnesis_core::{AreaId, ProjectId, UserId};

use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{MembershipQuery, MembershipRepository};

/// Grants `target` `role` on `area`. `actor_role` must already satisfy
/// `Action::ManageArea` (Project Admin or System Admin, resolved by the
/// caller *for this specific area* — see the module doc comment), and
/// `role` must not be `Role::SystemAdmin` (see the module doc comment).
pub async fn grant_area_role(
    repo: &dyn MembershipRepository,
    actor_role: Option<Role>,
    area: AreaId,
    target: &UserId,
    role: Role,
) -> Result<(), AppError> {
    if !is_allowed(actor_role, Action::ManageArea) {
        return Err(AppError::Forbidden);
    }
    if role == Role::SystemAdmin {
        return Err(AppError::Forbidden);
    }
    repo.set_area_role(target, area, role).await?;
    Ok(())
}

/// Revokes `target`'s role on `area` entirely. Same gate as
/// [`grant_area_role`].
pub async fn revoke_area_role(
    repo: &dyn MembershipRepository,
    actor_role: Option<Role>,
    area: AreaId,
    target: &UserId,
) -> Result<(), AppError> {
    if !is_allowed(actor_role, Action::ManageArea) {
        return Err(AppError::Forbidden);
    }
    repo.revoke_area_role(target, area).await?;
    Ok(())
}

/// Grants `target` `role` on `project`. `actor_role` must already satisfy
/// `Action::ManageProjectMembership` (Project Admin or System Admin,
/// resolved by the caller *for this specific project*), and `role` must not
/// be `Role::SystemAdmin` (see the module doc comment).
pub async fn grant_project_role(
    repo: &dyn MembershipRepository,
    actor_role: Option<Role>,
    project: ProjectId,
    target: &UserId,
    role: Role,
) -> Result<(), AppError> {
    if !is_allowed(actor_role, Action::ManageProjectMembership) {
        return Err(AppError::Forbidden);
    }
    if role == Role::SystemAdmin {
        return Err(AppError::Forbidden);
    }
    repo.set_project_role(target, project, role).await?;
    Ok(())
}

/// Revokes `target`'s role on `project` entirely. Same gate as
/// [`grant_project_role`].
pub async fn revoke_project_role(
    repo: &dyn MembershipRepository,
    actor_role: Option<Role>,
    project: ProjectId,
    target: &UserId,
) -> Result<(), AppError> {
    if !is_allowed(actor_role, Action::ManageProjectMembership) {
        return Err(AppError::Forbidden);
    }
    repo.revoke_project_role(target, project).await?;
    Ok(())
}

/// Grants `target` System Admin. `actor_role` must already satisfy
/// `Action::ManageUsers` — System Admin only, so only an existing System
/// Admin can ever create another one.
pub async fn grant_system_admin(
    repo: &dyn MembershipRepository,
    actor_role: Option<Role>,
    target: &UserId,
) -> Result<(), AppError> {
    if !is_allowed(actor_role, Action::ManageUsers) {
        return Err(AppError::Forbidden);
    }
    repo.grant_system_admin(target).await?;
    Ok(())
}

/// Revokes `target`'s System Admin. `actor_role` must already satisfy
/// `Action::ManageUsers`, and — see the module doc comment — this refuses
/// to revoke the very last System Admin in the system
/// (`AppError::LastSystemAdmin`) rather than leaving a self-hosted
/// deployment with nobody left who can administer it.
pub async fn revoke_system_admin(
    query: &dyn MembershipQuery,
    repo: &dyn MembershipRepository,
    actor_role: Option<Role>,
    target: &UserId,
) -> Result<(), AppError> {
    if !is_allowed(actor_role, Action::ManageUsers) {
        return Err(AppError::Forbidden);
    }
    if query.is_system_admin(target).await? {
        let admins = query.list_system_admins().await?;
        if admins.len() <= 1 {
            return Err(AppError::LastSystemAdmin);
        }
    }
    repo.revoke_system_admin(target).await?;
    Ok(())
}

/// Lists every explicit `(user, role)` grant on `area` — a System-Admin-or-
/// Project-Admin-only view (`Action::ManageArea`, the same tier that may
/// change these grants), not exposed to a plain Member.
pub async fn list_area_members(
    query: &dyn MembershipQuery,
    actor_role: Option<Role>,
    area: AreaId,
) -> Result<Vec<(UserId, Role)>, AppError> {
    if !is_allowed(actor_role, Action::ManageArea) {
        return Err(AppError::Forbidden);
    }
    Ok(query.list_area_members(area).await?)
}

/// Lists every explicit `(user, role)` grant on `project` — the
/// [`list_area_members`] sibling, gated on `Action::ManageProjectMembership`.
pub async fn list_project_members(
    query: &dyn MembershipQuery,
    actor_role: Option<Role>,
    project: ProjectId,
) -> Result<Vec<(UserId, Role)>, AppError> {
    if !is_allowed(actor_role, Action::ManageProjectMembership) {
        return Err(AppError::Forbidden);
    }
    Ok(query.list_project_members(project).await?)
}

/// Lists every current System Admin — gated on `Action::ManageUsers`, the
/// same tier that may grant or revoke it.
pub async fn list_system_admins(
    query: &dyn MembershipQuery,
    actor_role: Option<Role>,
) -> Result<Vec<UserId>, AppError> {
    if !is_allowed(actor_role, Action::ManageUsers) {
        return Err(AppError::Forbidden);
    }
    Ok(query.list_system_admins().await?)
}

#[cfg(test)]
mod tests {
    //! These tests live here (rather than only in `tests/domain_use_cases.rs`)
    //! because the module doc comment above states the exact security
    //! property they exist to prove — keeping them next to the rule under
    //! test is what makes "would this test actually fail if the check were
    //! removed" easy to verify by inspection alone. `tests/domain_use_cases.rs`
    //! adds the wider integration-shaped coverage (real `Fakes`, real
    //! `MembershipQuery` resolution across two distinct areas).

    use super::*;

    /// A minimal `MembershipRepository` + `MembershipQuery` fake, local to
    /// this module: writes are recorded, never actually interpreted — every
    /// test here asserts on *whether* a write was attempted, which is
    /// exactly what "was the escalation actually blocked before touching
    /// storage" needs, without pulling in the full `tests/domain_fakes`
    /// machinery this crate's own `#[cfg(test)]` code cannot depend on
    /// (that lives under `tests/`, a separate compilation unit).
    #[derive(Default)]
    struct Recorder {
        area_grants: std::sync::Mutex<Vec<(UserId, AreaId, Role)>>,
        project_grants: std::sync::Mutex<Vec<(UserId, ProjectId, Role)>>,
        system_admin_grants: std::sync::Mutex<Vec<UserId>>,
        system_admin_revocations: std::sync::Mutex<Vec<UserId>>,
        system_admins: Vec<UserId>,
    }

    #[async_trait::async_trait]
    impl MembershipRepository for Recorder {
        async fn grant_system_admin(&self, user: &UserId) -> Result<(), crate::error::RepoError> {
            self.system_admin_grants.lock().unwrap().push(user.clone());
            Ok(())
        }
        async fn revoke_system_admin(&self, user: &UserId) -> Result<(), crate::error::RepoError> {
            self.system_admin_revocations
                .lock()
                .unwrap()
                .push(user.clone());
            Ok(())
        }
        async fn set_area_role(
            &self,
            user: &UserId,
            area: AreaId,
            role: Role,
        ) -> Result<(), crate::error::RepoError> {
            self.area_grants
                .lock()
                .unwrap()
                .push((user.clone(), area, role));
            Ok(())
        }
        async fn revoke_area_role(
            &self,
            _user: &UserId,
            _area: AreaId,
        ) -> Result<(), crate::error::RepoError> {
            Ok(())
        }
        async fn set_project_role(
            &self,
            user: &UserId,
            project: ProjectId,
            role: Role,
        ) -> Result<(), crate::error::RepoError> {
            self.project_grants
                .lock()
                .unwrap()
                .push((user.clone(), project, role));
            Ok(())
        }
        async fn revoke_project_role(
            &self,
            _user: &UserId,
            _project: ProjectId,
        ) -> Result<(), crate::error::RepoError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl MembershipQuery for Recorder {
        async fn is_system_admin(&self, user: &UserId) -> Result<bool, crate::error::RepoError> {
            Ok(self.system_admins.contains(user))
        }
        async fn area_role(
            &self,
            _user: &UserId,
            _area: AreaId,
        ) -> Result<Option<Role>, crate::error::RepoError> {
            Ok(None)
        }
        async fn project_role(
            &self,
            _user: &UserId,
            _project: ProjectId,
        ) -> Result<Option<Role>, crate::error::RepoError> {
            Ok(None)
        }
        async fn list_system_admins(&self) -> Result<Vec<UserId>, crate::error::RepoError> {
            Ok(self.system_admins.clone())
        }
        async fn list_area_members(
            &self,
            _area: AreaId,
        ) -> Result<Vec<(UserId, Role)>, crate::error::RepoError> {
            Ok(vec![])
        }
        async fn list_project_members(
            &self,
            _project: ProjectId,
        ) -> Result<Vec<(UserId, Role)>, crate::error::RepoError> {
            Ok(vec![])
        }
    }

    fn area() -> AreaId {
        AreaId::new(uuid::Uuid::from_u128(1))
    }
    fn project() -> ProjectId {
        ProjectId::new(uuid::Uuid::from_u128(2))
    }
    fn alice() -> UserId {
        UserId::new("alice")
    }
    fn mallory() -> UserId {
        UserId::new("mallory")
    }

    // --- No privilege escalation: a Member cannot grant anything at all ---

    #[tokio::test]
    async fn a_member_cannot_grant_an_area_role() {
        let repo = Recorder::default();
        let result =
            grant_area_role(&repo, Some(Role::Member), area(), &mallory(), Role::Member).await;
        assert_eq!(result, Err(AppError::Forbidden));
        assert!(repo.area_grants.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_member_cannot_grant_a_project_role() {
        let repo = Recorder::default();
        let result = grant_project_role(
            &repo,
            Some(Role::Member),
            project(),
            &mallory(),
            Role::Member,
        )
        .await;
        assert_eq!(result, Err(AppError::Forbidden));
        assert!(repo.project_grants.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_member_cannot_grant_system_admin() {
        let repo = Recorder::default();
        let result = grant_system_admin(&repo, Some(Role::Member), &mallory()).await;
        assert_eq!(result, Err(AppError::Forbidden));
        assert!(repo.system_admin_grants.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn no_role_at_all_cannot_grant_anything() {
        let repo = Recorder::default();
        assert_eq!(
            grant_area_role(&repo, None, area(), &mallory(), Role::Member).await,
            Err(AppError::Forbidden)
        );
        assert_eq!(
            grant_project_role(&repo, None, project(), &mallory(), Role::Member).await,
            Err(AppError::Forbidden)
        );
        assert_eq!(
            grant_system_admin(&repo, None, &mallory()).await,
            Err(AppError::Forbidden)
        );
    }

    // --- No privilege escalation: a Project Admin cannot mint System Admin
    // through the Area/Project grant path, even though they legitimately
    // pass the ordinary ManageArea/ManageProjectMembership gate. ---

    #[tokio::test]
    async fn a_project_admin_cannot_grant_system_admin_via_an_area_role() {
        let repo = Recorder::default();
        let result = grant_area_role(
            &repo,
            Some(Role::ProjectAdmin),
            area(),
            &mallory(),
            Role::SystemAdmin,
        )
        .await;
        assert_eq!(
            result,
            Err(AppError::Forbidden),
            "granting SystemAdmin through an area role must be refused even to an admin \
             who legitimately manages that area"
        );
        assert!(
            repo.area_grants.lock().unwrap().is_empty(),
            "the write must never reach the repository"
        );
    }

    #[tokio::test]
    async fn a_project_admin_cannot_grant_system_admin_via_a_project_role() {
        let repo = Recorder::default();
        let result = grant_project_role(
            &repo,
            Some(Role::ProjectAdmin),
            project(),
            &mallory(),
            Role::SystemAdmin,
        )
        .await;
        assert_eq!(result, Err(AppError::Forbidden));
        assert!(repo.project_grants.lock().unwrap().is_empty());
    }

    /// Even a *System Admin* actor granting `Role::SystemAdmin` through the
    /// area/project path (rather than through `grant_system_admin`) is
    /// refused — the ban is on the write shape (a "system_admin" row in a
    /// scoped membership table, which nothing downstream is designed to
    /// interpret as real system-wide authority), not merely on who is
    /// asking. There is a real, ungated path for a System Admin to do this
    /// correctly: `grant_system_admin`.
    #[tokio::test]
    async fn even_a_system_admin_actor_cannot_write_system_admin_through_the_area_path() {
        let repo = Recorder::default();
        let result = grant_area_role(
            &repo,
            Some(Role::SystemAdmin),
            area(),
            &mallory(),
            Role::SystemAdmin,
        )
        .await;
        assert_eq!(result, Err(AppError::Forbidden));
        assert!(repo.area_grants.lock().unwrap().is_empty());
    }

    // --- No privilege escalation: an actor cannot grant a role on a scope
    // they do not administer (simulated here the same way the real system
    // resolves it: `actor_role` is the caller-resolved role *for that
    // specific scope*, so "does not administer this area" is exactly
    // `actor_role: None`). ---

    #[tokio::test]
    async fn granting_on_a_scope_the_actor_does_not_administer_is_refused() {
        let repo = Recorder::default();
        // Even though `mallory` genuinely administers *some* other area,
        // the caller resolved her role *for this area* as `None` -- exactly
        // what happens when a real caller resolves
        // `MembershipQuery::effective_area_role` for an area she holds no
        // grant on.
        let result = grant_area_role(&repo, None, area(), &alice(), Role::Member).await;
        assert_eq!(result, Err(AppError::Forbidden));
        assert!(repo.area_grants.lock().unwrap().is_empty());
    }

    // --- No privilege escalation: nobody can elevate their own role. This
    // is not a separate check -- it falls out of the two rules above, and
    // these tests exist to prove that composition actually holds for the
    // self-target case specifically. ---

    #[tokio::test]
    async fn a_member_cannot_grant_themselves_a_stronger_role() {
        let repo = Recorder::default();
        // Bob, a Member, tries to promote himself to Project Admin on the
        // same project.
        let bob = UserId::new("bob");
        let result = grant_project_role(
            &repo,
            Some(Role::Member),
            project(),
            &bob,
            Role::ProjectAdmin,
        )
        .await;
        assert_eq!(result, Err(AppError::Forbidden));
        assert!(repo.project_grants.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_project_admin_cannot_escalate_themselves_to_system_admin() {
        let repo = Recorder::default();
        let priya = UserId::new("priya");
        let result = grant_project_role(
            &repo,
            Some(Role::ProjectAdmin),
            project(),
            &priya,
            Role::SystemAdmin,
        )
        .await;
        assert_eq!(result, Err(AppError::Forbidden));
        assert!(repo.project_grants.lock().unwrap().is_empty());
    }

    // --- Legitimate grants still work, at every tier that should work. ---

    #[tokio::test]
    async fn a_project_admin_can_grant_a_member_role_on_their_own_project() {
        let repo = Recorder::default();
        grant_project_role(
            &repo,
            Some(Role::ProjectAdmin),
            project(),
            &alice(),
            Role::Member,
        )
        .await
        .unwrap();
        assert_eq!(
            repo.project_grants.lock().unwrap().as_slice(),
            &[(alice(), project(), Role::Member)]
        );
    }

    #[tokio::test]
    async fn a_system_admin_can_grant_system_admin() {
        let repo = Recorder::default();
        grant_system_admin(&repo, Some(Role::SystemAdmin), &alice())
            .await
            .unwrap();
        assert_eq!(
            repo.system_admin_grants.lock().unwrap().as_slice(),
            &[alice()]
        );
    }

    // --- The last System Admin ---

    #[tokio::test]
    async fn revoking_the_last_system_admin_is_refused() {
        let repo = Recorder {
            system_admins: vec![alice()],
            ..Recorder::default()
        };
        let result = revoke_system_admin(&repo, &repo, Some(Role::SystemAdmin), &alice()).await;
        assert_eq!(result, Err(AppError::LastSystemAdmin));
        assert!(
            repo.system_admin_revocations.lock().unwrap().is_empty(),
            "the write must never reach the repository"
        );
    }

    #[tokio::test]
    async fn revoking_a_system_admin_when_another_remains_succeeds() {
        let bob = UserId::new("bob");
        let repo = Recorder {
            system_admins: vec![alice(), bob.clone()],
            ..Recorder::default()
        };
        revoke_system_admin(&repo, &repo, Some(Role::SystemAdmin), &alice())
            .await
            .unwrap();
        assert_eq!(
            repo.system_admin_revocations.lock().unwrap().as_slice(),
            &[alice()]
        );
    }

    #[tokio::test]
    async fn revoking_system_admin_from_someone_who_does_not_hold_it_is_a_harmless_no_op() {
        let repo = Recorder {
            system_admins: vec![alice()],
            ..Recorder::default()
        };
        // `mallory` is not a System Admin at all -- the last-admin check
        // only applies to the target actually holding it.
        revoke_system_admin(&repo, &repo, Some(Role::SystemAdmin), &mallory())
            .await
            .unwrap();
        assert_eq!(
            repo.system_admin_revocations.lock().unwrap().as_slice(),
            &[mallory()]
        );
    }

    #[tokio::test]
    async fn only_a_system_admin_may_revoke_system_admin() {
        let repo = Recorder {
            system_admins: vec![alice()],
            ..Recorder::default()
        };
        let result = revoke_system_admin(&repo, &repo, Some(Role::ProjectAdmin), &alice()).await;
        assert_eq!(result, Err(AppError::Forbidden));
        assert!(repo.system_admin_revocations.lock().unwrap().is_empty());
    }
}
