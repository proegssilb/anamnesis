//! Who a project's own members can `@`-mention (issue #44's picker):
//! deliberately a *different*, lower permission tier than
//! [`crate::use_cases::user_directory::list_known_users`]'s "everyone this
//! deployment has ever seen" — mentioning a project's own people is
//! ordinary task work, not a structural/admin action, so it is gated at
//! [`Action::ViewProject`] (any Member) rather than [`Action::ManageArea`].
//!
//! "The project's own people" means everyone with actual access to it, not
//! only the users [`MembershipQuery::list_project_members`] and
//! [`MembershipQuery::list_area_members`] name directly: a role can also
//! reach a project through a mapped group ([`GroupMembershipQuery`]), and
//! those members are just as real a mention target — arguably more likely
//! to be one, since group-mapped access is how a whole team gets onto a
//! project at once. [`list_mentionable_users`] unions both dimensions the
//! same way `crate::access` unions them for a permission check, except here
//! the result is "who", not "how much".
//!
//! System Admin is deliberately excluded from this union, even though a
//! System Admin can view (and so could mention on) any project: enumerating
//! who holds System Admin is its own, more sensitive listing
//! ([`crate::use_cases::membership::list_system_admins`]), gated at
//! [`Action::ManageUsers`] — this use case must not become a side door to
//! that same information at a much weaker gate.

use std::collections::HashSet;

use anamnesis_core::policy::Role;
use anamnesis_core::{AreaId, ProjectId, UserId};

use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{GroupMembershipQuery, MembershipQuery, UserDirectoryQuery};

/// Every user who can be `@`-mentioned on a task in `project` (which lives
/// in `area`): its direct members, its area's direct members, and everyone
/// who reaches either through a mapped group — each resolved to their
/// last-seen display name, falling back to the raw id for anyone who has
/// never logged in.
pub async fn list_mentionable_users(
    membership: &dyn MembershipQuery,
    groups: &dyn GroupMembershipQuery,
    directory: &dyn UserDirectoryQuery,
    actor_role: Option<Role>,
    project: ProjectId,
    area: AreaId,
) -> Result<Vec<(UserId, String)>, AppError> {
    if !is_allowed(actor_role, Action::ViewProject) {
        return Err(AppError::Forbidden);
    }
    let ids = collect_mentionable_ids(membership, groups, project, area).await?;
    let names = directory.display_names(&ids).await?;
    Ok(ids
        .into_iter()
        .map(|id| {
            let name = names.get(&id).cloned().unwrap_or_else(|| id.to_string());
            (id, name)
        })
        .collect())
}

/// The id half of [`list_mentionable_users`]: every user directly on
/// `project` or `area`, plus every user reached through a group mapped onto
/// either — split out so the permission check and the name-resolution step
/// stay the only two things in the public function, matching this crate's
/// usual shape of "gate, gather, resolve".
async fn collect_mentionable_ids(
    membership: &dyn MembershipQuery,
    groups: &dyn GroupMembershipQuery,
    project: ProjectId,
    area: AreaId,
) -> Result<Vec<UserId>, AppError> {
    let mut ids: HashSet<UserId> = HashSet::new();
    for (user, _) in membership.list_project_members(project).await? {
        ids.insert(user);
    }
    for (user, _) in membership.list_area_members(area).await? {
        ids.insert(user);
    }
    for (group, _) in groups.list_project_groups(project).await? {
        ids.extend(groups.list_users_in_group(&group).await?);
    }
    for (group, _) in groups.list_area_groups(area).await? {
        ids.extend(groups.list_users_in_group(&group).await?);
    }
    let mut ids: Vec<UserId> = ids.into_iter().collect();
    ids.sort();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::error::RepoError;

    #[derive(Default)]
    struct Fixture {
        project_members: Vec<(UserId, Role)>,
        area_members: Vec<(UserId, Role)>,
        project_groups: Vec<(String, Role)>,
        area_groups: Vec<(String, Role)>,
        group_users: HashMap<String, Vec<UserId>>,
        names: HashMap<UserId, String>,
    }

    #[async_trait::async_trait]
    impl MembershipQuery for Fixture {
        async fn is_system_admin(&self, _user: &UserId) -> Result<bool, RepoError> {
            Ok(false)
        }
        async fn area_role(
            &self,
            _user: &UserId,
            _area: AreaId,
        ) -> Result<Option<Role>, RepoError> {
            Ok(None)
        }
        async fn project_role(
            &self,
            _user: &UserId,
            _project: ProjectId,
        ) -> Result<Option<Role>, RepoError> {
            Ok(None)
        }
        async fn list_system_admins(&self) -> Result<Vec<UserId>, RepoError> {
            Ok(Vec::new())
        }
        async fn list_area_members(&self, _area: AreaId) -> Result<Vec<(UserId, Role)>, RepoError> {
            Ok(self.area_members.clone())
        }
        async fn list_project_members(
            &self,
            _project: ProjectId,
        ) -> Result<Vec<(UserId, Role)>, RepoError> {
            Ok(self.project_members.clone())
        }
    }

    #[async_trait::async_trait]
    impl GroupMembershipQuery for Fixture {
        async fn is_system_admin_via_group(&self, _user: &UserId) -> Result<bool, RepoError> {
            Ok(false)
        }
        async fn area_group_role(
            &self,
            _user: &UserId,
            _area: AreaId,
        ) -> Result<Option<Role>, RepoError> {
            Ok(None)
        }
        async fn project_group_role(
            &self,
            _user: &UserId,
            _project: ProjectId,
        ) -> Result<Option<Role>, RepoError> {
            Ok(None)
        }
        async fn list_admin_groups(&self) -> Result<Vec<String>, RepoError> {
            Ok(Vec::new())
        }
        async fn list_area_groups(&self, _area: AreaId) -> Result<Vec<(String, Role)>, RepoError> {
            Ok(self.area_groups.clone())
        }
        async fn list_project_groups(
            &self,
            _project: ProjectId,
        ) -> Result<Vec<(String, Role)>, RepoError> {
            Ok(self.project_groups.clone())
        }
        async fn list_known_groups(&self) -> Result<Vec<String>, RepoError> {
            Ok(Vec::new())
        }
        async fn list_users_in_group(&self, group: &str) -> Result<Vec<UserId>, RepoError> {
            Ok(self.group_users.get(group).cloned().unwrap_or_default())
        }
    }

    #[async_trait::async_trait]
    impl UserDirectoryQuery for Fixture {
        async fn display_names(
            &self,
            users: &[UserId],
        ) -> Result<HashMap<UserId, String>, RepoError> {
            Ok(users
                .iter()
                .filter_map(|u| self.names.get(u).map(|n| (u.clone(), n.clone())))
                .collect())
        }
        async fn list_known_users(&self) -> Result<Vec<(UserId, String)>, RepoError> {
            Ok(Vec::new())
        }
    }

    fn ids() -> (ProjectId, AreaId) {
        (
            ProjectId::new(uuid::Uuid::from_u128(1)),
            AreaId::new(uuid::Uuid::from_u128(2)),
        )
    }

    #[tokio::test]
    async fn a_stranger_with_no_role_at_all_is_refused() {
        let fixture = Fixture::default();
        let (project, area) = ids();
        let result =
            list_mentionable_users(&fixture, &fixture, &fixture, None, project, area).await;
        assert_eq!(result, Err(AppError::Forbidden));
    }

    #[tokio::test]
    async fn a_plain_member_can_list_mentionable_users() {
        let alice = UserId::new("alice");
        let mut names = HashMap::new();
        names.insert(alice.clone(), "Alice".to_string());
        let fixture = Fixture {
            project_members: vec![(alice.clone(), Role::Member)],
            names,
            ..Default::default()
        };
        let (project, area) = ids();
        let result = list_mentionable_users(
            &fixture,
            &fixture,
            &fixture,
            Some(Role::Member),
            project,
            area,
        )
        .await
        .unwrap();
        assert_eq!(result, vec![(alice, "Alice".to_string())]);
    }

    #[tokio::test]
    async fn direct_area_and_project_members_and_both_groups_are_all_included_once_each() {
        let direct_project = UserId::new("direct-project");
        let direct_area = UserId::new("direct-area");
        let via_project_group = UserId::new("via-project-group");
        let via_area_group = UserId::new("via-area-group");
        // In both a project-mapped and an area-mapped group -- must appear
        // only once in the result, not twice.
        let in_both_groups = UserId::new("in-both-groups");

        let mut group_users = HashMap::new();
        group_users.insert(
            "project-team".to_string(),
            vec![via_project_group.clone(), in_both_groups.clone()],
        );
        group_users.insert(
            "area-team".to_string(),
            vec![via_area_group.clone(), in_both_groups.clone()],
        );

        let fixture = Fixture {
            project_members: vec![(direct_project.clone(), Role::Member)],
            area_members: vec![(direct_area.clone(), Role::Member)],
            project_groups: vec![("project-team".to_string(), Role::Member)],
            area_groups: vec![("area-team".to_string(), Role::Member)],
            group_users,
            ..Default::default()
        };
        let (project, area) = ids();
        let result = list_mentionable_users(
            &fixture,
            &fixture,
            &fixture,
            Some(Role::Member),
            project,
            area,
        )
        .await
        .unwrap();

        let mut got: Vec<UserId> = result.into_iter().map(|(id, _)| id).collect();
        got.sort();
        let mut expected = vec![
            direct_project,
            direct_area,
            via_project_group,
            via_area_group,
            in_both_groups,
        ];
        expected.sort();
        assert_eq!(got, expected);
    }

    #[tokio::test]
    async fn a_user_with_no_recorded_login_falls_back_to_their_raw_id() {
        let ghost = UserId::new("never-logged-in");
        let fixture = Fixture {
            project_members: vec![(ghost.clone(), Role::Member)],
            ..Default::default()
        };
        let (project, area) = ids();
        let result = list_mentionable_users(
            &fixture,
            &fixture,
            &fixture,
            Some(Role::Member),
            project,
            area,
        )
        .await
        .unwrap();
        assert_eq!(result, vec![(ghost.clone(), ghost.to_string())]);
    }
}
