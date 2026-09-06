//! [`UserDirectoryQuery`]/[`UserDirectoryRepository`] over [`SqlStore`]: the
//! best-effort display-name cache in `0005_known_users.sql`.

use std::collections::HashMap;

use anamnesis_app::{RepoError, UserDirectoryQuery, UserDirectoryRepository};
use anamnesis_core::UserId;
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::{Backend, SqlStore};

mod sqlite_impl {
    use super::*;

    pub(super) async fn display_name(
        pool: &SqlitePool,
        user: &UserId,
    ) -> Result<Option<String>, RepoError> {
        let row = sqlx::query("SELECT display_name FROM known_users WHERE user_id = ?")
            .bind(user.as_str())
            .fetch_optional(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to look up a known user", e))?;
        Ok(row.map(|r| r.get("display_name")))
    }

    pub(super) async fn list_known_users(
        pool: &SqlitePool,
    ) -> Result<Vec<(UserId, String)>, RepoError> {
        let rows =
            sqlx::query("SELECT user_id, display_name FROM known_users ORDER BY display_name")
                .fetch_all(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to list known users", e))?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    UserId::new(r.get::<String, _>("user_id")),
                    r.get("display_name"),
                )
            })
            .collect())
    }

    pub(super) async fn remember(
        pool: &SqlitePool,
        user: &UserId,
        display_name: &str,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO known_users (user_id, display_name) VALUES (?, ?) \
             ON CONFLICT(user_id) DO UPDATE SET display_name = excluded.display_name",
        )
        .bind(user.as_str())
        .bind(display_name)
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to record a known user", e))?;
        Ok(())
    }
}

mod postgres_impl {
    use super::*;

    pub(super) async fn display_name(
        pool: &PgPool,
        user: &UserId,
    ) -> Result<Option<String>, RepoError> {
        let row = sqlx::query("SELECT display_name FROM known_users WHERE user_id = $1")
            .bind(user.as_str())
            .fetch_optional(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to look up a known user", e))?;
        Ok(row.map(|r| r.get("display_name")))
    }

    pub(super) async fn list_known_users(
        pool: &PgPool,
    ) -> Result<Vec<(UserId, String)>, RepoError> {
        let rows =
            sqlx::query("SELECT user_id, display_name FROM known_users ORDER BY display_name")
                .fetch_all(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to list known users", e))?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    UserId::new(r.get::<String, _>("user_id")),
                    r.get("display_name"),
                )
            })
            .collect())
    }

    pub(super) async fn remember(
        pool: &PgPool,
        user: &UserId,
        display_name: &str,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO known_users (user_id, display_name) VALUES ($1, $2) \
             ON CONFLICT (user_id) DO UPDATE SET display_name = excluded.display_name",
        )
        .bind(user.as_str())
        .bind(display_name)
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to record a known user", e))?;
        Ok(())
    }
}

#[async_trait]
impl UserDirectoryQuery for SqlStore {
    /// One lookup per distinct id in `users` — this cache has no natural
    /// batch shape to build a dynamic `IN (...)` list around (unlike, say,
    /// `search_documents`), and every real caller passes a page-scoped list
    /// (a task's comments, an area's or project's members), never the whole
    /// deployment.
    async fn display_names(&self, users: &[UserId]) -> Result<HashMap<UserId, String>, RepoError> {
        let mut unique: Vec<&UserId> = Vec::new();
        for user in users {
            if !unique.contains(&user) {
                unique.push(user);
            }
        }
        let mut resolved = HashMap::new();
        for user in unique {
            let found = match &self.backend {
                Backend::Sqlite(pool) => sqlite_impl::display_name(pool, user).await?,
                Backend::Postgres(pool) => postgres_impl::display_name(pool, user).await?,
            };
            if let Some(name) = found {
                resolved.insert(user.clone(), name);
            }
        }
        Ok(resolved)
    }

    async fn list_known_users(&self) -> Result<Vec<(UserId, String)>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_known_users(pool).await,
            Backend::Postgres(pool) => postgres_impl::list_known_users(pool).await,
        }
    }
}

#[async_trait]
impl UserDirectoryRepository for SqlStore {
    async fn remember(&self, user: &UserId, display_name: &str) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::remember(pool, user, display_name).await,
            Backend::Postgres(pool) => postgres_impl::remember(pool, user, display_name).await,
        }
    }
}
