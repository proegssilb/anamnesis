//! Mapping identity-provider groups to roles — the group-scoped sibling of
//! [`crate::use_cases::membership`], and the only way a group ever comes to
//! grant anything.
//!
//! ## No privilege escalation
//!
//! Identical doctrine to [`crate::use_cases::membership`], for identical
//! reasons — the content being written can itself grant capability:
//!
//! - **System Admin can only ever be granted to a group through
//!   [`grant_admin_group`]**, itself gated on `Action::ManageUsers` (System
//!   Admin only). [`grant_area_group_role`] and [`grant_project_group_role`]
//!   both reject `role == Role::SystemAdmin` outright, before touching the
//!   repository, even when the actor *is* a System Admin. A Project Admin
//!   legitimately passes `Action::ManageArea`, and must not be able to reach
//!   system-wide authority by writing `"system_admin"` into an area-scoped
//!   group mapping and then joining that group.
//! - **A mapping is checked against the actor's role on that specific
//!   scope**, resolved by the caller through `crate::access` — so a Project
//!   Admin of one Area can never map a group onto a different Area.
//!
//! The group dimension makes the first rule *more* load-bearing than it is
//! for per-user grants, not less: a user grant names one known person,
//! whereas a group mapping applies to everyone the identity provider ever
//! puts in that group, including people added later by someone with no
//! anamnesis account at all.
//!
//! ## No last-admin check
//!
//! [`revoke_admin_group`] deliberately has no equivalent of
//! [`crate::use_cases::membership::revoke_system_admin`]'s
//! `AppError::LastSystemAdmin` guard. That check answers "would this leave
//! nobody able to administer the deployment", and a count of admin-group
//! *mappings* cannot answer it: a mapping is not evidence that any user is
//! in that group, and anamnesis never enumerates the identity provider's
//! users, so it cannot find out. Refusing to unmap the last group would
//! block a legitimate cleanup while still guaranteeing nothing. The per-user
//! check over `system_admins` is untouched and remains the real guard.

use anamnesis_core::policy::Role;
use anamnesis_core::{AreaId, ProjectId};

use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{GroupMembershipQuery, GroupMembershipRepository};

/// Maps `group` to `role` on `area`. `actor_role` must already satisfy
/// `Action::ManageArea` (resolved for this specific area), and `role` must
/// not be `Role::SystemAdmin` — see the module doc comment.
pub async fn grant_area_group_role(
    repo: &dyn GroupMembershipRepository,
    actor_role: Option<Role>,
    area: AreaId,
    group: &str,
    role: Role,
) -> Result<(), AppError> {
    if !is_allowed(actor_role, Action::ManageArea) {
        return Err(AppError::Forbidden);
    }
    if role == Role::SystemAdmin {
        return Err(AppError::Forbidden);
    }
    repo.set_area_group_role(group, area, role).await?;
    Ok(())
}

/// Unmaps `group` from `area` entirely. Same gate as
/// [`grant_area_group_role`].
pub async fn revoke_area_group_role(
    repo: &dyn GroupMembershipRepository,
    actor_role: Option<Role>,
    area: AreaId,
    group: &str,
) -> Result<(), AppError> {
    if !is_allowed(actor_role, Action::ManageArea) {
        return Err(AppError::Forbidden);
    }
    repo.revoke_area_group_role(group, area).await?;
    Ok(())
}

/// Maps `group` to `role` on `project`. `actor_role` must already satisfy
/// `Action::ManageProjectMembership` (resolved for this specific project),
/// and `role` must not be `Role::SystemAdmin`.
pub async fn grant_project_group_role(
    repo: &dyn GroupMembershipRepository,
    actor_role: Option<Role>,
    project: ProjectId,
    group: &str,
    role: Role,
) -> Result<(), AppError> {
    if !is_allowed(actor_role, Action::ManageProjectMembership) {
        return Err(AppError::Forbidden);
    }
    if role == Role::SystemAdmin {
        return Err(AppError::Forbidden);
    }
    repo.set_project_group_role(group, project, role).await?;
    Ok(())
}

/// Unmaps `group` from `project` entirely. Same gate as
/// [`grant_project_group_role`].
pub async fn revoke_project_group_role(
    repo: &dyn GroupMembershipRepository,
    actor_role: Option<Role>,
    project: ProjectId,
    group: &str,
) -> Result<(), AppError> {
    if !is_allowed(actor_role, Action::ManageProjectMembership) {
        return Err(AppError::Forbidden);
    }
    repo.revoke_project_group_role(group, project).await?;
    Ok(())
}

/// Maps `group` to System Admin. `actor_role` must satisfy
/// `Action::ManageUsers` — System Admin only.
pub async fn grant_admin_group(
    repo: &dyn GroupMembershipRepository,
    actor_role: Option<Role>,
    group: &str,
) -> Result<(), AppError> {
    if !is_allowed(actor_role, Action::ManageUsers) {
        return Err(AppError::Forbidden);
    }
    repo.grant_admin_group(group).await?;
    Ok(())
}

/// Unmaps `group` from System Admin. Same gate as [`grant_admin_group`]; see
/// the module doc comment for why there is no last-admin check.
pub async fn revoke_admin_group(
    repo: &dyn GroupMembershipRepository,
    actor_role: Option<Role>,
    group: &str,
) -> Result<(), AppError> {
    if !is_allowed(actor_role, Action::ManageUsers) {
        return Err(AppError::Forbidden);
    }
    repo.revoke_admin_group(group).await?;
    Ok(())
}

/// Lists every group mapped to System Admin — gated on
/// `Action::ManageUsers`, the same tier that may change those mappings.
pub async fn list_admin_groups(
    query: &dyn GroupMembershipQuery,
    actor_role: Option<Role>,
) -> Result<Vec<String>, AppError> {
    if !is_allowed(actor_role, Action::ManageUsers) {
        return Err(AppError::Forbidden);
    }
    Ok(query.list_admin_groups().await?)
}

/// Lists every `(group, role)` mapping on `area` — gated on
/// `Action::ManageArea`, matching
/// [`crate::use_cases::membership::list_area_members`].
pub async fn list_area_groups(
    query: &dyn GroupMembershipQuery,
    actor_role: Option<Role>,
    area: AreaId,
) -> Result<Vec<(String, Role)>, AppError> {
    if !is_allowed(actor_role, Action::ManageArea) {
        return Err(AppError::Forbidden);
    }
    Ok(query.list_area_groups(area).await?)
}

/// Lists every `(group, role)` mapping on `project` — gated on
/// `Action::ManageProjectMembership`.
pub async fn list_project_groups(
    query: &dyn GroupMembershipQuery,
    actor_role: Option<Role>,
    project: ProjectId,
) -> Result<Vec<(String, Role)>, AppError> {
    if !is_allowed(actor_role, Action::ManageProjectMembership) {
        return Err(AppError::Forbidden);
    }
    Ok(query.list_project_groups(project).await?)
}

/// Lists every group name anamnesis has seen, for the admin UI's group
/// pickers. Gated at the weakest tier that can create *any* mapping
/// (`Action::ManageArea`), since a Project Admin needs it to fill in an
/// area- or project-scoped mapping form.
pub async fn list_known_groups(
    query: &dyn GroupMembershipQuery,
    actor_role: Option<Role>,
) -> Result<Vec<String>, AppError> {
    if !is_allowed(actor_role, Action::ManageArea) {
        return Err(AppError::Forbidden);
    }
    Ok(query.list_known_groups().await?)
}

#[cfg(test)]
mod tests {
    //! As in [`crate::use_cases::membership`]'s own tests, these live beside
    //! the rules they prove: the module doc comment above states the exact
    //! security property, and a reader should be able to confirm by
    //! inspection that removing a check would fail a test here.

    use super::*;
    use anamnesis_core::UserId;

    #[derive(Default)]
    struct Recorder {
        area_mappings: std::sync::Mutex<Vec<(String, AreaId, Role)>>,
        project_mappings: std::sync::Mutex<Vec<(String, ProjectId, Role)>>,
        admin_mappings: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl GroupMembershipRepository for Recorder {
        async fn replace_user_groups(
            &self,
            _user: &UserId,
            _groups: &[String],
        ) -> Result<(), crate::error::RepoError> {
            Ok(())
        }
        async fn grant_admin_group(&self, group: &str) -> Result<(), crate::error::RepoError> {
            self.admin_mappings.lock().unwrap().push(group.to_string());
            Ok(())
        }
        async fn revoke_admin_group(&self, _group: &str) -> Result<(), crate::error::RepoError> {
            Ok(())
        }
        async fn set_area_group_role(
            &self,
            group: &str,
            area: AreaId,
            role: Role,
        ) -> Result<(), crate::error::RepoError> {
            self.area_mappings
                .lock()
                .unwrap()
                .push((group.to_string(), area, role));
            Ok(())
        }
        async fn revoke_area_group_role(
            &self,
            _group: &str,
            _area: AreaId,
        ) -> Result<(), crate::error::RepoError> {
            Ok(())
        }
        async fn set_project_group_role(
            &self,
            group: &str,
            project: ProjectId,
            role: Role,
        ) -> Result<(), crate::error::RepoError> {
            self.project_mappings
                .lock()
                .unwrap()
                .push((group.to_string(), project, role));
            Ok(())
        }
        async fn revoke_project_group_role(
            &self,
            _group: &str,
            _project: ProjectId,
        ) -> Result<(), crate::error::RepoError> {
            Ok(())
        }
    }

    fn area() -> AreaId {
        AreaId::new(uuid::Uuid::from_u128(1))
    }

    fn project() -> ProjectId {
        ProjectId::new(uuid::Uuid::from_u128(2))
    }

    #[tokio::test]
    async fn an_area_group_mapping_refuses_system_admin_even_for_a_system_admin() {
        let repo = Recorder::default();
        let err = grant_area_group_role(
            &repo,
            Some(Role::SystemAdmin),
            area(),
            "anamnesis-admins",
            Role::SystemAdmin,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, AppError::Forbidden));
        assert!(
            repo.area_mappings.lock().unwrap().is_empty(),
            "the escalation must be refused before it reaches storage"
        );
    }

    #[tokio::test]
    async fn a_project_group_mapping_refuses_system_admin_even_for_a_system_admin() {
        let repo = Recorder::default();
        let err = grant_project_group_role(
            &repo,
            Some(Role::SystemAdmin),
            project(),
            "anamnesis-admins",
            Role::SystemAdmin,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, AppError::Forbidden));
        assert!(repo.project_mappings.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_project_admin_cannot_map_a_group_to_system_admin() {
        let repo = Recorder::default();

        // Not through the area/project doors (rejected on the role)...
        assert!(matches!(
            grant_area_group_role(
                &repo,
                Some(Role::ProjectAdmin),
                area(),
                "g",
                Role::SystemAdmin
            )
            .await,
            Err(AppError::Forbidden)
        ));
        // ...nor through the front door (rejected on the actor).
        assert!(matches!(
            grant_admin_group(&repo, Some(Role::ProjectAdmin), "g").await,
            Err(AppError::Forbidden)
        ));
        assert!(repo.admin_mappings.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_project_admin_can_map_a_group_to_the_grantable_roles() {
        let repo = Recorder::default();
        grant_area_group_role(&repo, Some(Role::ProjectAdmin), area(), "g", Role::Member)
            .await
            .unwrap();
        grant_project_group_role(
            &repo,
            Some(Role::ProjectAdmin),
            project(),
            "g",
            Role::ProjectAdmin,
        )
        .await
        .unwrap();

        assert_eq!(
            *repo.area_mappings.lock().unwrap(),
            vec![("g".to_string(), area(), Role::Member)]
        );
        assert_eq!(
            *repo.project_mappings.lock().unwrap(),
            vec![("g".to_string(), project(), Role::ProjectAdmin)]
        );
    }

    #[tokio::test]
    async fn a_member_cannot_map_a_group_anywhere() {
        let repo = Recorder::default();
        for result in [
            grant_area_group_role(&repo, Some(Role::Member), area(), "g", Role::Member).await,
            revoke_area_group_role(&repo, Some(Role::Member), area(), "g").await,
            grant_project_group_role(&repo, Some(Role::Member), project(), "g", Role::Member).await,
            revoke_project_group_role(&repo, Some(Role::Member), project(), "g").await,
            grant_admin_group(&repo, Some(Role::Member), "g").await,
            revoke_admin_group(&repo, Some(Role::Member), "g").await,
        ] {
            assert!(matches!(result, Err(AppError::Forbidden)));
        }
        assert!(repo.area_mappings.lock().unwrap().is_empty());
        assert!(repo.project_mappings.lock().unwrap().is_empty());
        assert!(repo.admin_mappings.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_user_with_no_role_at_all_cannot_map_a_group_anywhere() {
        let repo = Recorder::default();
        for result in [
            grant_area_group_role(&repo, None, area(), "g", Role::Member).await,
            grant_project_group_role(&repo, None, project(), "g", Role::Member).await,
            grant_admin_group(&repo, None, "g").await,
        ] {
            assert!(matches!(result, Err(AppError::Forbidden)));
        }
    }

    #[tokio::test]
    async fn only_a_system_admin_can_map_a_group_to_system_admin() {
        let repo = Recorder::default();
        grant_admin_group(&repo, Some(Role::SystemAdmin), "anamnesis-admins")
            .await
            .unwrap();
        assert_eq!(
            *repo.admin_mappings.lock().unwrap(),
            vec!["anamnesis-admins".to_string()]
        );
    }

    #[tokio::test]
    async fn unmapping_the_last_admin_group_is_allowed() {
        // Unlike revoking the last System Admin *user*: see the module doc
        // comment. A mapping is not evidence anyone holds admin, so refusing
        // here would block a cleanup while guaranteeing nothing.
        let repo = Recorder::default();
        revoke_admin_group(&repo, Some(Role::SystemAdmin), "anamnesis-admins")
            .await
            .unwrap();
    }
}
