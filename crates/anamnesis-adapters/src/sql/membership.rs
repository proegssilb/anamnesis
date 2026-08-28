//! [`MembershipQuery`] and [`MembershipRepository`] over [`SqlStore`]
//! (`docs/DOMAIN.md` §7's Area-scoped roles pass): `system_admins` for
//! global admins, `area_members` and `project_members` for the two scoped
//! role tables.
//!
//! [`MembershipQuery::effective_area_role`] and
//! [`MembershipQuery::effective_role`] are default trait methods (defined
//! once, in `anamnesis_app::ports::membership`, in terms of
//! [`MembershipQuery::is_system_admin`], [`MembershipQuery::area_role`], and
//! [`MembershipQuery::project_role`]) — this module only implements those
//! three primitives, never overrides the composed ones.
//!
//! [`MembershipRepository`]'s grant methods (`grant_system_admin`,
//! `set_area_role`, `set_project_role`) started life as inherent seams on
//! `SqlStore` — the only way anything above bootstrap could ever write a
//! grant, since Phase D shipped no write port at all. This module promotes
//! them into the real port, alongside the revoke methods and the listing
//! queries (`list_system_admins`/`list_area_members`/`list_project_members`)
//! that make an actual "who has access" UI possible.

use anamnesis_app::{MembershipQuery, MembershipRepository, RepoError};
use anamnesis_core::{AreaId, ProjectId, UserId, policy::Role};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::{Backend, SqlStore};

fn role_to_text(role: Role) -> &'static str {
    match role {
        Role::SystemAdmin => "system_admin",
        Role::ProjectAdmin => "project_admin",
        Role::Member => "member",
    }
}

fn role_from_text(raw: &str) -> Result<Role, RepoError> {
    match raw {
        "system_admin" => Ok(Role::SystemAdmin),
        "project_admin" => Ok(Role::ProjectAdmin),
        "member" => Ok(Role::Member),
        other => Err(RepoError::new(format!("invalid stored role {other:?}"))),
    }
}

mod sqlite_impl {
    use super::*;

    pub(super) async fn is_system_admin(
        pool: &SqlitePool,
        user: &UserId,
    ) -> Result<bool, RepoError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT user_id FROM system_admins WHERE user_id = ?")
                .bind(user.as_str())
                .fetch_optional(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to check system admin", e))?;
        Ok(row.is_some())
    }

    pub(super) async fn area_role(
        pool: &SqlitePool,
        user: &UserId,
        area: AreaId,
    ) -> Result<Option<Role>, RepoError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT role FROM area_members WHERE user_id = ? AND area_id = ?")
                .bind(user.as_str())
                .bind(area.as_uuid().to_string())
                .fetch_optional(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to load area role", e))?;
        row.map(|(r,)| role_from_text(&r)).transpose()
    }

    pub(super) async fn project_role(
        pool: &SqlitePool,
        user: &UserId,
        project: ProjectId,
    ) -> Result<Option<Role>, RepoError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT role FROM project_members WHERE user_id = ? AND project_id = ?")
                .bind(user.as_str())
                .bind(project.as_uuid().to_string())
                .fetch_optional(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to load project role", e))?;
        row.map(|(r,)| role_from_text(&r)).transpose()
    }

    pub(super) async fn list_system_admins(pool: &SqlitePool) -> Result<Vec<UserId>, RepoError> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT user_id FROM system_admins")
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list system admins", e))?;
        Ok(rows.into_iter().map(|(u,)| UserId::new(u)).collect())
    }

    pub(super) async fn list_area_members(
        pool: &SqlitePool,
        area: AreaId,
    ) -> Result<Vec<(UserId, Role)>, RepoError> {
        let rows = sqlx::query("SELECT user_id, role FROM area_members WHERE area_id = ?")
            .bind(area.as_uuid().to_string())
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list area members", e))?;
        rows.into_iter()
            .map(|row| {
                let role = role_from_text(&row.get::<String, _>("role"))?;
                Ok((UserId::new(row.get::<String, _>("user_id")), role))
            })
            .collect()
    }

    pub(super) async fn list_project_members(
        pool: &SqlitePool,
        project: ProjectId,
    ) -> Result<Vec<(UserId, Role)>, RepoError> {
        let rows = sqlx::query("SELECT user_id, role FROM project_members WHERE project_id = ?")
            .bind(project.as_uuid().to_string())
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list project members", e))?;
        rows.into_iter()
            .map(|row| {
                let role = role_from_text(&row.get::<String, _>("role"))?;
                Ok((UserId::new(row.get::<String, _>("user_id")), role))
            })
            .collect()
    }

    pub(super) async fn revoke_system_admin(
        pool: &SqlitePool,
        user: &UserId,
    ) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM system_admins WHERE user_id = ?")
            .bind(user.as_str())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to revoke system admin", e))?;
        Ok(())
    }

    pub(super) async fn revoke_area_role(
        pool: &SqlitePool,
        user: &UserId,
        area: AreaId,
    ) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM area_members WHERE user_id = ? AND area_id = ?")
            .bind(user.as_str())
            .bind(area.as_uuid().to_string())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to revoke area role", e))?;
        Ok(())
    }

    pub(super) async fn revoke_project_role(
        pool: &SqlitePool,
        user: &UserId,
        project: ProjectId,
    ) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM project_members WHERE user_id = ? AND project_id = ?")
            .bind(user.as_str())
            .bind(project.as_uuid().to_string())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to revoke project role", e))?;
        Ok(())
    }
}

mod postgres_impl {
    use super::*;

    pub(super) async fn is_system_admin(pool: &PgPool, user: &UserId) -> Result<bool, RepoError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT user_id FROM system_admins WHERE user_id = $1")
                .bind(user.as_str())
                .fetch_optional(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to check system admin", e))?;
        Ok(row.is_some())
    }

    pub(super) async fn area_role(
        pool: &PgPool,
        user: &UserId,
        area: AreaId,
    ) -> Result<Option<Role>, RepoError> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT role FROM area_members WHERE user_id = $1 AND area_id = $2")
                .bind(user.as_str())
                .bind(area.as_uuid())
                .fetch_optional(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to load area role", e))?;
        row.map(|(r,)| role_from_text(&r)).transpose()
    }

    pub(super) async fn project_role(
        pool: &PgPool,
        user: &UserId,
        project: ProjectId,
    ) -> Result<Option<Role>, RepoError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT role FROM project_members WHERE user_id = $1 AND project_id = $2",
        )
        .bind(user.as_str())
        .bind(project.as_uuid())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load project role", e))?;
        row.map(|(r,)| role_from_text(&r)).transpose()
    }

    pub(super) async fn list_system_admins(pool: &PgPool) -> Result<Vec<UserId>, RepoError> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT user_id FROM system_admins")
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list system admins", e))?;
        Ok(rows.into_iter().map(|(u,)| UserId::new(u)).collect())
    }

    pub(super) async fn list_area_members(
        pool: &PgPool,
        area: AreaId,
    ) -> Result<Vec<(UserId, Role)>, RepoError> {
        let rows = sqlx::query("SELECT user_id, role FROM area_members WHERE area_id = $1")
            .bind(area.as_uuid())
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list area members", e))?;
        rows.into_iter()
            .map(|row| {
                let role = role_from_text(&row.get::<String, _>("role"))?;
                Ok((UserId::new(row.get::<String, _>("user_id")), role))
            })
            .collect()
    }

    pub(super) async fn list_project_members(
        pool: &PgPool,
        project: ProjectId,
    ) -> Result<Vec<(UserId, Role)>, RepoError> {
        let rows = sqlx::query("SELECT user_id, role FROM project_members WHERE project_id = $1")
            .bind(project.as_uuid())
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list project members", e))?;
        rows.into_iter()
            .map(|row| {
                let role = role_from_text(&row.get::<String, _>("role"))?;
                Ok((UserId::new(row.get::<String, _>("user_id")), role))
            })
            .collect()
    }

    pub(super) async fn revoke_system_admin(pool: &PgPool, user: &UserId) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM system_admins WHERE user_id = $1")
            .bind(user.as_str())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to revoke system admin", e))?;
        Ok(())
    }

    pub(super) async fn revoke_area_role(
        pool: &PgPool,
        user: &UserId,
        area: AreaId,
    ) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM area_members WHERE user_id = $1 AND area_id = $2")
            .bind(user.as_str())
            .bind(area.as_uuid())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to revoke area role", e))?;
        Ok(())
    }

    pub(super) async fn revoke_project_role(
        pool: &PgPool,
        user: &UserId,
        project: ProjectId,
    ) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM project_members WHERE user_id = $1 AND project_id = $2")
            .bind(user.as_str())
            .bind(project.as_uuid())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to revoke project role", e))?;
        Ok(())
    }
}

#[async_trait]
impl MembershipQuery for SqlStore {
    async fn is_system_admin(&self, user: &UserId) -> Result<bool, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::is_system_admin(pool, user).await,
            Backend::Postgres(pool) => postgres_impl::is_system_admin(pool, user).await,
        }
    }

    async fn area_role(&self, user: &UserId, area: AreaId) -> Result<Option<Role>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::area_role(pool, user, area).await,
            Backend::Postgres(pool) => postgres_impl::area_role(pool, user, area).await,
        }
    }

    async fn project_role(
        &self,
        user: &UserId,
        project: ProjectId,
    ) -> Result<Option<Role>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::project_role(pool, user, project).await,
            Backend::Postgres(pool) => postgres_impl::project_role(pool, user, project).await,
        }
    }

    async fn list_system_admins(&self) -> Result<Vec<UserId>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_system_admins(pool).await,
            Backend::Postgres(pool) => postgres_impl::list_system_admins(pool).await,
        }
    }

    async fn list_area_members(&self, area: AreaId) -> Result<Vec<(UserId, Role)>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_area_members(pool, area).await,
            Backend::Postgres(pool) => postgres_impl::list_area_members(pool, area).await,
        }
    }

    async fn list_project_members(
        &self,
        project: ProjectId,
    ) -> Result<Vec<(UserId, Role)>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_project_members(pool, project).await,
            Backend::Postgres(pool) => postgres_impl::list_project_members(pool, project).await,
        }
    }
}

#[async_trait]
impl MembershipRepository for SqlStore {
    async fn grant_system_admin(&self, user: &UserId) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlx::query("INSERT OR IGNORE INTO system_admins (user_id) VALUES (?)")
                    .bind(user.as_str())
                    .execute(pool)
                    .await
                    .map_err(|e| RepoError::from_source("failed to grant system admin", e))?;
            }
            Backend::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO system_admins (user_id) VALUES ($1) ON CONFLICT DO NOTHING",
                )
                .bind(user.as_str())
                .execute(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to grant system admin", e))?;
            }
        }
        Ok(())
    }

    async fn revoke_system_admin(&self, user: &UserId) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::revoke_system_admin(pool, user).await,
            Backend::Postgres(pool) => postgres_impl::revoke_system_admin(pool, user).await,
        }
    }

    async fn set_area_role(
        &self,
        user: &UserId,
        area: AreaId,
        role: Role,
    ) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlx::query(
                    "INSERT INTO area_members (user_id, area_id, role) VALUES (?, ?, ?) \
                     ON CONFLICT(user_id, area_id) DO UPDATE SET role = excluded.role",
                )
                .bind(user.as_str())
                .bind(area.as_uuid().to_string())
                .bind(role_to_text(role))
                .execute(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to set area role", e))?;
            }
            Backend::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO area_members (user_id, area_id, role) VALUES ($1, $2, $3) \
                     ON CONFLICT (user_id, area_id) DO UPDATE SET role = excluded.role",
                )
                .bind(user.as_str())
                .bind(area.as_uuid())
                .bind(role_to_text(role))
                .execute(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to set area role", e))?;
            }
        }
        Ok(())
    }

    async fn revoke_area_role(&self, user: &UserId, area: AreaId) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::revoke_area_role(pool, user, area).await,
            Backend::Postgres(pool) => postgres_impl::revoke_area_role(pool, user, area).await,
        }
    }

    async fn set_project_role(
        &self,
        user: &UserId,
        project: ProjectId,
        role: Role,
    ) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlx::query(
                    "INSERT INTO project_members (user_id, project_id, role) VALUES (?, ?, ?) \
                     ON CONFLICT(user_id, project_id) DO UPDATE SET role = excluded.role",
                )
                .bind(user.as_str())
                .bind(project.as_uuid().to_string())
                .bind(role_to_text(role))
                .execute(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to set project role", e))?;
            }
            Backend::Postgres(pool) => {
                sqlx::query(
                    "INSERT INTO project_members (user_id, project_id, role) VALUES ($1, $2, $3) \
                     ON CONFLICT (user_id, project_id) DO UPDATE SET role = excluded.role",
                )
                .bind(user.as_str())
                .bind(project.as_uuid())
                .bind(role_to_text(role))
                .execute(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to set project role", e))?;
            }
        }
        Ok(())
    }

    async fn revoke_project_role(
        &self,
        user: &UserId,
        project: ProjectId,
    ) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::revoke_project_role(pool, user, project).await,
            Backend::Postgres(pool) => {
                postgres_impl::revoke_project_role(pool, user, project).await
            }
        }
    }
}
