//! The one place the two sources of role membership are combined.
//!
//! [`crate::ports::MembershipQuery`] answers "what do this user's own grants
//! give them here"; [`crate::ports::GroupMembershipQuery`] answers the same
//! question for the groups their identity provider asserted at login. Each
//! composes its own dimension internally, by `docs/DOMAIN.md`'s
//! strongest-grant-wins rule. The three functions here take the `.max()` of
//! the two answers — the same rule once more, applied across dimensions.
//!
//! **Every caller should use these**, never a port's own `effective_*`
//! method directly: a bare [`crate::ports::MembershipQuery::effective_role`]
//! is only half the answer, and would wrongly deny a user whose whole access
//! comes through a group. Keeping the join here means there is exactly one
//! site to audit and exactly one site to change.
//!
//! Deliberately free functions rather than another trait: this is
//! composition over two ports, not a third thing to implement, and there is
//! no sensible adapter for it. Groups are inert when the feature is
//! unconfigured (no recorded membership, no mappings), so on a deployment
//! that never sets `ANAMNESIS_OIDC_GROUPS_CLAIM` these reduce to exactly the
//! per-user answer they returned before groups existed.

use anamnesis_core::policy::Role;
use anamnesis_core::{AreaId, ProjectId, UserId};

use crate::error::RepoError;
use crate::ports::{GroupMembershipQuery, MembershipQuery};

/// Whether `user` holds System Admin — directly, or through any group of
/// theirs mapped to it.
pub async fn is_system_admin(
    users: &dyn MembershipQuery,
    groups: &dyn GroupMembershipQuery,
    user: &UserId,
) -> Result<bool, RepoError> {
    Ok(users.is_system_admin(user).await? || groups.is_system_admin_via_group(user).await?)
}

/// `user`'s effective role on `area`: the stronger of what they hold
/// directly and what their groups hold.
///
/// This is what a purely area-scoped action (`ViewArea`, `ManageArea`) and
/// `CreateProject` should be gated on.
pub async fn effective_area_role(
    users: &dyn MembershipQuery,
    groups: &dyn GroupMembershipQuery,
    user: &UserId,
    area: AreaId,
) -> Result<Option<Role>, RepoError> {
    let by_user = users.effective_area_role(user, area).await?;
    let by_group = groups.effective_area_role(user, area).await?;
    Ok(by_user.max(by_group))
}

/// `user`'s effective role on `project`, which lives in `area`: the stronger
/// of what they hold directly and what their groups hold.
///
/// This is what every project-scoped action should be gated on.
pub async fn effective_role(
    users: &dyn MembershipQuery,
    groups: &dyn GroupMembershipQuery,
    user: &UserId,
    project: ProjectId,
    area: AreaId,
) -> Result<Option<Role>, RepoError> {
    let by_user = users.effective_role(user, project, area).await?;
    let by_group = groups.effective_role(user, project, area).await?;
    Ok(by_user.max(by_group))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    /// A user who holds exactly `user_role` directly and `group_role`
    /// through a group, on every scope, so each test names only the pair it
    /// is composing.
    struct Grants {
        user_role: Option<Role>,
        group_role: Option<Role>,
    }

    #[async_trait]
    impl MembershipQuery for Grants {
        async fn is_system_admin(&self, _: &UserId) -> Result<bool, RepoError> {
            Ok(self.user_role == Some(Role::SystemAdmin))
        }
        async fn area_role(&self, _: &UserId, _: AreaId) -> Result<Option<Role>, RepoError> {
            Ok(self.user_role)
        }
        async fn project_role(&self, _: &UserId, _: ProjectId) -> Result<Option<Role>, RepoError> {
            Ok(self.user_role)
        }
        async fn list_system_admins(&self) -> Result<Vec<UserId>, RepoError> {
            Ok(vec![])
        }
        async fn list_area_members(&self, _: AreaId) -> Result<Vec<(UserId, Role)>, RepoError> {
            Ok(vec![])
        }
        async fn list_project_members(
            &self,
            _: ProjectId,
        ) -> Result<Vec<(UserId, Role)>, RepoError> {
            Ok(vec![])
        }
    }

    #[async_trait]
    impl GroupMembershipQuery for Grants {
        async fn is_system_admin_via_group(&self, _: &UserId) -> Result<bool, RepoError> {
            Ok(self.group_role == Some(Role::SystemAdmin))
        }
        async fn area_group_role(&self, _: &UserId, _: AreaId) -> Result<Option<Role>, RepoError> {
            Ok(self.group_role)
        }
        async fn project_group_role(
            &self,
            _: &UserId,
            _: ProjectId,
        ) -> Result<Option<Role>, RepoError> {
            Ok(self.group_role)
        }
        async fn list_admin_groups(&self) -> Result<Vec<String>, RepoError> {
            Ok(vec![])
        }
        async fn list_area_groups(&self, _: AreaId) -> Result<Vec<(String, Role)>, RepoError> {
            Ok(vec![])
        }
        async fn list_project_groups(
            &self,
            _: ProjectId,
        ) -> Result<Vec<(String, Role)>, RepoError> {
            Ok(vec![])
        }
        async fn list_known_groups(&self) -> Result<Vec<String>, RepoError> {
            Ok(vec![])
        }
    }

    fn user() -> UserId {
        UserId::new("u")
    }

    fn area() -> AreaId {
        AreaId::new(uuid::Uuid::from_u128(1))
    }

    fn project() -> ProjectId {
        ProjectId::new(uuid::Uuid::from_u128(2))
    }

    async fn area_role_for(user_role: Option<Role>, group_role: Option<Role>) -> Option<Role> {
        let g = Grants {
            user_role,
            group_role,
        };
        effective_area_role(&g, &g, &user(), area()).await.unwrap()
    }

    async fn project_role_for(user_role: Option<Role>, group_role: Option<Role>) -> Option<Role> {
        let g = Grants {
            user_role,
            group_role,
        };
        effective_role(&g, &g, &user(), project(), area())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn neither_dimension_granting_is_no_access() {
        assert_eq!(area_role_for(None, None).await, None);
        assert_eq!(project_role_for(None, None).await, None);
    }

    #[tokio::test]
    async fn a_group_grant_alone_is_enough() {
        assert_eq!(
            area_role_for(None, Some(Role::ProjectAdmin)).await,
            Some(Role::ProjectAdmin)
        );
        assert_eq!(
            project_role_for(None, Some(Role::Member)).await,
            Some(Role::Member)
        );
    }

    #[tokio::test]
    async fn a_user_grant_alone_still_works_with_groups_unconfigured() {
        assert_eq!(
            area_role_for(Some(Role::ProjectAdmin), None).await,
            Some(Role::ProjectAdmin)
        );
        assert_eq!(
            project_role_for(Some(Role::Member), None).await,
            Some(Role::Member)
        );
    }

    #[tokio::test]
    async fn the_stronger_grant_wins_from_either_dimension() {
        // A group grant never demotes a stronger direct grant...
        assert_eq!(
            area_role_for(Some(Role::ProjectAdmin), Some(Role::Member)).await,
            Some(Role::ProjectAdmin)
        );
        // ...and a direct grant never caps a stronger group grant.
        assert_eq!(
            area_role_for(Some(Role::Member), Some(Role::ProjectAdmin)).await,
            Some(Role::ProjectAdmin)
        );
        assert_eq!(
            project_role_for(Some(Role::Member), Some(Role::ProjectAdmin)).await,
            Some(Role::ProjectAdmin)
        );
    }

    #[tokio::test]
    async fn system_admin_is_held_through_either_dimension() {
        let direct = Grants {
            user_role: Some(Role::SystemAdmin),
            group_role: None,
        };
        let via_group = Grants {
            user_role: None,
            group_role: Some(Role::SystemAdmin),
        };
        let neither = Grants {
            user_role: Some(Role::ProjectAdmin),
            group_role: Some(Role::ProjectAdmin),
        };

        assert!(is_system_admin(&direct, &direct, &user()).await.unwrap());
        assert!(
            is_system_admin(&via_group, &via_group, &user())
                .await
                .unwrap()
        );
        assert!(!is_system_admin(&neither, &neither, &user()).await.unwrap());
    }

    #[tokio::test]
    async fn a_system_admin_group_carries_into_every_scope() {
        assert_eq!(
            area_role_for(None, Some(Role::SystemAdmin)).await,
            Some(Role::SystemAdmin)
        );
        assert_eq!(
            project_role_for(None, Some(Role::SystemAdmin)).await,
            Some(Role::SystemAdmin)
        );
    }
}
