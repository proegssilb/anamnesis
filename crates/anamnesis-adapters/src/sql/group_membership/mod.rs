//! [`GroupMembershipQuery`] and [`GroupMembershipRepository`] over
//! [`SqlStore`]: the optional group dimension of authorization, in four
//! tables added by `0004_group_membership.sql`.
//!
//! `user_groups` caches what the identity provider last said about a user,
//! rewritten wholesale at each login. The other three
//! (`system_admin_groups`, `area_group_members`, `project_group_members`)
//! are the mappings a System Admin creates, and are the only thing that
//! turns a group into a grant — a `user_groups` row on its own confers
//! nothing.
//!
//! Every role lookup is therefore one join from `user_groups` to the
//! matching mapping table, keyed on `group_name`. A user can be in several
//! mapped groups at once, so each lookup collects *all* matching roles and
//! returns the strongest; the ordering lives in `Role`'s `Ord`, never in the
//! SQL, because the stored text happens to sort into the right order by
//! coincidence and relying on that would silently break the moment a role is
//! renamed.
//!
//! As in [`super::membership`], the composed `effective_*` methods are
//! default trait methods defined once in `anamnesis_app::ports`; this module
//! implements only the primitives, split into [`sqlite_impl`] and
//! [`postgres_impl`] the same way [`super::membership`] is.

mod postgres_impl;
mod sqlite_impl;

use anamnesis_app::{GroupMembershipQuery, GroupMembershipRepository, RepoError};
use anamnesis_core::{AreaId, ProjectId, UserId, policy::Role};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::membership::{role_from_text, role_to_text};
use super::{Backend, SqlStore};

/// The strongest of the roles a user reaches through their mapped groups, or
/// `None` when they reach none. See the module doc comment for why the
/// ordering is applied here rather than in SQL.
fn strongest_role(raw: Vec<String>) -> Result<Option<Role>, RepoError> {
    let mut strongest = None;
    for text in raw {
        strongest = strongest.max(Some(role_from_text(&text)?));
    }
    Ok(strongest)
}

/// Turns rows of `(group_name, role)` into the pairs the listing methods
/// return, failing on any unparseable stored role.
fn group_role_pairs<R: Row>(rows: Vec<R>) -> Result<Vec<(String, Role)>, RepoError>
where
    for<'a> String: sqlx::Decode<'a, R::Database> + sqlx::Type<R::Database>,
    for<'a> &'a str: sqlx::ColumnIndex<R>,
{
    rows.into_iter()
        .map(|row| {
            let role = role_from_text(&row.get::<String, _>("role"))?;
            Ok((row.get::<String, _>("group_name"), role))
        })
        .collect()
}

#[async_trait]
impl GroupMembershipQuery for SqlStore {
    async fn is_system_admin_via_group(&self, user: &UserId) -> Result<bool, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::is_system_admin_via_group(pool, user).await,
            Backend::Postgres(pool) => postgres_impl::is_system_admin_via_group(pool, user).await,
        }
    }

    async fn area_group_role(
        &self,
        user: &UserId,
        area: AreaId,
    ) -> Result<Option<Role>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::area_group_role(pool, user, area).await,
            Backend::Postgres(pool) => postgres_impl::area_group_role(pool, user, area).await,
        }
    }

    async fn project_group_role(
        &self,
        user: &UserId,
        project: ProjectId,
    ) -> Result<Option<Role>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::project_group_role(pool, user, project).await,
            Backend::Postgres(pool) => postgres_impl::project_group_role(pool, user, project).await,
        }
    }

    async fn list_admin_groups(&self) -> Result<Vec<String>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_admin_groups(pool).await,
            Backend::Postgres(pool) => postgres_impl::list_admin_groups(pool).await,
        }
    }

    async fn list_area_groups(&self, area: AreaId) -> Result<Vec<(String, Role)>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_area_groups(pool, area).await,
            Backend::Postgres(pool) => postgres_impl::list_area_groups(pool, area).await,
        }
    }

    async fn list_project_groups(
        &self,
        project: ProjectId,
    ) -> Result<Vec<(String, Role)>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_project_groups(pool, project).await,
            Backend::Postgres(pool) => postgres_impl::list_project_groups(pool, project).await,
        }
    }

    async fn list_known_groups(&self) -> Result<Vec<String>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_known_groups(pool).await,
            Backend::Postgres(pool) => postgres_impl::list_known_groups(pool).await,
        }
    }
}

#[async_trait]
impl GroupMembershipRepository for SqlStore {
    async fn replace_user_groups(&self, user: &UserId, groups: &[String]) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::replace_user_groups(pool, user, groups).await,
            Backend::Postgres(pool) => postgres_impl::replace_user_groups(pool, user, groups).await,
        }
    }

    async fn grant_admin_group(&self, group: &str) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::grant_admin_group(pool, group).await,
            Backend::Postgres(pool) => postgres_impl::grant_admin_group(pool, group).await,
        }
    }

    async fn revoke_admin_group(&self, group: &str) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::revoke_admin_group(pool, group).await,
            Backend::Postgres(pool) => postgres_impl::revoke_admin_group(pool, group).await,
        }
    }

    async fn set_area_group_role(
        &self,
        group: &str,
        area: AreaId,
        role: Role,
    ) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlite_impl::set_area_group_role(pool, group, area, role).await
            }
            Backend::Postgres(pool) => {
                postgres_impl::set_area_group_role(pool, group, area, role).await
            }
        }
    }

    async fn revoke_area_group_role(&self, group: &str, area: AreaId) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::revoke_area_group_role(pool, group, area).await,
            Backend::Postgres(pool) => {
                postgres_impl::revoke_area_group_role(pool, group, area).await
            }
        }
    }

    async fn set_project_group_role(
        &self,
        group: &str,
        project: ProjectId,
        role: Role,
    ) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlite_impl::set_project_group_role(pool, group, project, role).await
            }
            Backend::Postgres(pool) => {
                postgres_impl::set_project_group_role(pool, group, project, role).await
            }
        }
    }

    async fn revoke_project_group_role(
        &self,
        group: &str,
        project: ProjectId,
    ) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlite_impl::revoke_project_group_role(pool, group, project).await
            }
            Backend::Postgres(pool) => {
                postgres_impl::revoke_project_group_role(pool, group, project).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strongest_group_role_wins_regardless_of_row_order() {
        // A user in several mapped groups holds the strongest role any of
        // them confers — the same `chmod` rule the per-user dimension uses,
        // applied within the group dimension.
        let ascending = vec!["member".to_string(), "project_admin".to_string()];
        let descending = vec!["project_admin".to_string(), "member".to_string()];
        assert_eq!(strongest_role(ascending).unwrap(), Some(Role::ProjectAdmin));
        assert_eq!(
            strongest_role(descending).unwrap(),
            Some(Role::ProjectAdmin)
        );
    }

    #[test]
    fn no_mapped_groups_is_no_role() {
        assert_eq!(strongest_role(Vec::new()).unwrap(), None);
    }

    #[test]
    fn an_unparseable_stored_role_is_an_error_not_a_silent_skip() {
        assert!(strongest_role(vec!["wizard".to_string()]).is_err());
    }
}
