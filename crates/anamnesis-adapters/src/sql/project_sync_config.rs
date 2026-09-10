//! [`ProjectSyncConfigRepository`] over [`SqlStore`]: at most one row per
//! project (issues #40/#41).

use anamnesis_app::{ProjectSyncConfig, ProjectSyncConfigRepository, RepoError};
use anamnesis_core::{ProjectId, Timestamp};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::{
    Backend, SqlStore, parse_uuid, sync_provider_from_text, sync_provider_to_text,
    timestamp_from_seconds,
};

#[allow(clippy::too_many_arguments)]
fn assemble(
    project_id: uuid::Uuid,
    provider: String,
    base_url: Option<String>,
    owner: String,
    repo: String,
    encrypted_token: Vec<u8>,
    enabled: bool,
    auto_import_new_issues: bool,
    auto_push_new_tasks: bool,
    created_at: i64,
    updated_at: i64,
    last_synced_at: Option<i64>,
    last_sync_error: Option<String>,
) -> Result<ProjectSyncConfig, RepoError> {
    Ok(ProjectSyncConfig {
        project_id: ProjectId::new(project_id),
        provider: sync_provider_from_text(&provider)?,
        base_url,
        owner,
        repo,
        encrypted_token,
        enabled,
        auto_import_new_issues,
        auto_push_new_tasks,
        created_at: timestamp_from_seconds(created_at)?,
        updated_at: timestamp_from_seconds(updated_at)?,
        last_synced_at: last_synced_at.map(timestamp_from_seconds).transpose()?,
        last_sync_error,
    })
}

mod sqlite_impl {
    use super::*;

    pub(super) async fn load(
        pool: &SqlitePool,
        project_id: ProjectId,
    ) -> Result<Option<ProjectSyncConfig>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT project_id, provider, base_url, owner, repo, encrypted_token, enabled, \
             auto_import_new_issues, auto_push_new_tasks, created_at, updated_at, \
             last_synced_at, last_sync_error \
             FROM project_sync_configs WHERE project_id = ?",
        )
        .bind(project_id.as_uuid().to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load project sync config", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble_row(&row)?))
    }

    pub(super) async fn list_enabled(
        pool: &SqlitePool,
    ) -> Result<Vec<ProjectSyncConfig>, RepoError> {
        let rows = sqlx::query(
            "SELECT project_id, provider, base_url, owner, repo, encrypted_token, enabled, \
             auto_import_new_issues, auto_push_new_tasks, created_at, updated_at, \
             last_synced_at, last_sync_error \
             FROM project_sync_configs WHERE enabled = 1",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list enabled project sync configs", e))?;
        rows.iter().map(assemble_row).collect()
    }

    fn assemble_row(row: &sqlx::sqlite::SqliteRow) -> Result<ProjectSyncConfig, RepoError> {
        assemble(
            parse_uuid(&row.get::<String, _>("project_id"))?,
            row.get("provider"),
            row.get("base_url"),
            row.get("owner"),
            row.get("repo"),
            row.get("encrypted_token"),
            row.get::<i64, _>("enabled") != 0,
            row.get::<i64, _>("auto_import_new_issues") != 0,
            row.get::<i64, _>("auto_push_new_tasks") != 0,
            row.get::<i64, _>("created_at"),
            row.get::<i64, _>("updated_at"),
            row.get::<Option<i64>, _>("last_synced_at"),
            row.get("last_sync_error"),
        )
    }

    pub(super) async fn upsert(
        pool: &SqlitePool,
        config: &ProjectSyncConfig,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO project_sync_configs \
             (project_id, provider, base_url, owner, repo, encrypted_token, enabled, \
              auto_import_new_issues, auto_push_new_tasks, created_at, updated_at, \
              last_synced_at, last_sync_error) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(project_id) DO UPDATE SET \
               provider = excluded.provider, base_url = excluded.base_url, \
               owner = excluded.owner, repo = excluded.repo, \
               encrypted_token = excluded.encrypted_token, enabled = excluded.enabled, \
               auto_import_new_issues = excluded.auto_import_new_issues, \
               auto_push_new_tasks = excluded.auto_push_new_tasks, \
               updated_at = excluded.updated_at",
        )
        .bind(config.project_id.as_uuid().to_string())
        .bind(sync_provider_to_text(config.provider))
        .bind(&config.base_url)
        .bind(&config.owner)
        .bind(&config.repo)
        .bind(&config.encrypted_token)
        .bind(config.enabled)
        .bind(config.auto_import_new_issues)
        .bind(config.auto_push_new_tasks)
        .bind(config.created_at.unix_seconds())
        .bind(config.updated_at.unix_seconds())
        .bind(config.last_synced_at.map(|t| t.unix_seconds()))
        .bind(&config.last_sync_error)
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to upsert project sync config", e))?;
        Ok(())
    }

    pub(super) async fn record_sync_result(
        pool: &SqlitePool,
        project_id: ProjectId,
        synced_at: Timestamp,
        error: Option<&str>,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "UPDATE project_sync_configs SET last_synced_at = ?, last_sync_error = ? \
             WHERE project_id = ?",
        )
        .bind(synced_at.unix_seconds())
        .bind(error)
        .bind(project_id.as_uuid().to_string())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to record a sync result", e))?;
        Ok(())
    }

    pub(super) async fn delete(pool: &SqlitePool, project_id: ProjectId) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM project_sync_configs WHERE project_id = ?")
            .bind(project_id.as_uuid().to_string())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to delete project sync config", e))?;
        Ok(())
    }
}

mod postgres_impl {
    use super::*;

    pub(super) async fn load(
        pool: &PgPool,
        project_id: ProjectId,
    ) -> Result<Option<ProjectSyncConfig>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT project_id, provider, base_url, owner, repo, encrypted_token, enabled, \
             auto_import_new_issues, auto_push_new_tasks, created_at, updated_at, \
             last_synced_at, last_sync_error \
             FROM project_sync_configs WHERE project_id = $1",
        )
        .bind(project_id.as_uuid())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load project sync config", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble_row(&row)?))
    }

    pub(super) async fn list_enabled(pool: &PgPool) -> Result<Vec<ProjectSyncConfig>, RepoError> {
        let rows = sqlx::query(
            "SELECT project_id, provider, base_url, owner, repo, encrypted_token, enabled, \
             auto_import_new_issues, auto_push_new_tasks, created_at, updated_at, \
             last_synced_at, last_sync_error \
             FROM project_sync_configs WHERE enabled",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list enabled project sync configs", e))?;
        rows.iter().map(assemble_row).collect()
    }

    fn assemble_row(row: &sqlx::postgres::PgRow) -> Result<ProjectSyncConfig, RepoError> {
        assemble(
            row.get::<uuid::Uuid, _>("project_id"),
            row.get("provider"),
            row.get("base_url"),
            row.get("owner"),
            row.get("repo"),
            row.get("encrypted_token"),
            row.get("enabled"),
            row.get("auto_import_new_issues"),
            row.get("auto_push_new_tasks"),
            row.get::<i64, _>("created_at"),
            row.get::<i64, _>("updated_at"),
            row.get::<Option<i64>, _>("last_synced_at"),
            row.get("last_sync_error"),
        )
    }

    pub(super) async fn upsert(pool: &PgPool, config: &ProjectSyncConfig) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO project_sync_configs \
             (project_id, provider, base_url, owner, repo, encrypted_token, enabled, \
              auto_import_new_issues, auto_push_new_tasks, created_at, updated_at, \
              last_synced_at, last_sync_error) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
             ON CONFLICT (project_id) DO UPDATE SET \
               provider = excluded.provider, base_url = excluded.base_url, \
               owner = excluded.owner, repo = excluded.repo, \
               encrypted_token = excluded.encrypted_token, enabled = excluded.enabled, \
               auto_import_new_issues = excluded.auto_import_new_issues, \
               auto_push_new_tasks = excluded.auto_push_new_tasks, \
               updated_at = excluded.updated_at",
        )
        .bind(config.project_id.as_uuid())
        .bind(sync_provider_to_text(config.provider))
        .bind(&config.base_url)
        .bind(&config.owner)
        .bind(&config.repo)
        .bind(&config.encrypted_token)
        .bind(config.enabled)
        .bind(config.auto_import_new_issues)
        .bind(config.auto_push_new_tasks)
        .bind(config.created_at.unix_seconds())
        .bind(config.updated_at.unix_seconds())
        .bind(config.last_synced_at.map(|t| t.unix_seconds()))
        .bind(&config.last_sync_error)
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to upsert project sync config", e))?;
        Ok(())
    }

    pub(super) async fn record_sync_result(
        pool: &PgPool,
        project_id: ProjectId,
        synced_at: Timestamp,
        error: Option<&str>,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "UPDATE project_sync_configs SET last_synced_at = $1, last_sync_error = $2 \
             WHERE project_id = $3",
        )
        .bind(synced_at.unix_seconds())
        .bind(error)
        .bind(project_id.as_uuid())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to record a sync result", e))?;
        Ok(())
    }

    pub(super) async fn delete(pool: &PgPool, project_id: ProjectId) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM project_sync_configs WHERE project_id = $1")
            .bind(project_id.as_uuid())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to delete project sync config", e))?;
        Ok(())
    }
}

#[async_trait]
impl ProjectSyncConfigRepository for SqlStore {
    async fn load(&self, project_id: ProjectId) -> Result<Option<ProjectSyncConfig>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::load(pool, project_id).await,
            Backend::Postgres(pool) => postgres_impl::load(pool, project_id).await,
        }
    }

    async fn list_enabled(&self) -> Result<Vec<ProjectSyncConfig>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_enabled(pool).await,
            Backend::Postgres(pool) => postgres_impl::list_enabled(pool).await,
        }
    }

    async fn upsert(&self, config: &ProjectSyncConfig) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::upsert(pool, config).await,
            Backend::Postgres(pool) => postgres_impl::upsert(pool, config).await,
        }
    }

    async fn record_sync_result(
        &self,
        project_id: ProjectId,
        synced_at: Timestamp,
        error: Option<&str>,
    ) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlite_impl::record_sync_result(pool, project_id, synced_at, error).await
            }
            Backend::Postgres(pool) => {
                postgres_impl::record_sync_result(pool, project_id, synced_at, error).await
            }
        }
    }

    async fn delete(&self, project_id: ProjectId) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::delete(pool, project_id).await,
            Backend::Postgres(pool) => postgres_impl::delete(pool, project_id).await,
        }
    }
}
