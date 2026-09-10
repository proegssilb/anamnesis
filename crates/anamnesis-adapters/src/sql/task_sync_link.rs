//! [`TaskSyncLinkRepository`] over [`SqlStore`]: one row per Task <-> external
//! Issue link (issues #40/#41).

use anamnesis_app::{RepoError, TaskSyncLink, TaskSyncLinkRepository};
use anamnesis_core::{ProjectId, TaskId};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::{Backend, SqlStore, parse_uuid, timestamp_from_seconds};

#[allow(clippy::too_many_arguments)]
fn assemble(
    task_id: uuid::Uuid,
    project_id: uuid::Uuid,
    external_issue_number: i64,
    external_url: String,
    last_remote_updated_at: i64,
    last_local_synced_at: i64,
    last_comment_synced_at: Option<i64>,
    created_at: i64,
) -> Result<TaskSyncLink, RepoError> {
    let external_issue_number = u64::try_from(external_issue_number)
        .map_err(|e| RepoError::from_source("invalid stored external issue number", e))?;
    Ok(TaskSyncLink {
        task_id: TaskId::new(task_id),
        project_id: ProjectId::new(project_id),
        external_issue_number,
        external_url,
        last_remote_updated_at: timestamp_from_seconds(last_remote_updated_at)?,
        last_local_synced_at: timestamp_from_seconds(last_local_synced_at)?,
        last_comment_synced_at: last_comment_synced_at.map(timestamp_from_seconds).transpose()?,
        created_at: timestamp_from_seconds(created_at)?,
    })
}

mod sqlite_impl {
    use super::*;

    fn assemble_row(row: &sqlx::sqlite::SqliteRow) -> Result<TaskSyncLink, RepoError> {
        assemble(
            parse_uuid(&row.get::<String, _>("task_id"))?,
            parse_uuid(&row.get::<String, _>("project_id"))?,
            row.get::<i64, _>("external_issue_number"),
            row.get("external_url"),
            row.get::<i64, _>("last_remote_updated_at"),
            row.get::<i64, _>("last_local_synced_at"),
            row.get::<Option<i64>, _>("last_comment_synced_at"),
            row.get::<i64, _>("created_at"),
        )
    }

    pub(super) async fn load_by_task(
        pool: &SqlitePool,
        task_id: TaskId,
    ) -> Result<Option<TaskSyncLink>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT task_id, project_id, external_issue_number, external_url, \
             last_remote_updated_at, last_local_synced_at, last_comment_synced_at, created_at \
             FROM task_sync_links WHERE task_id = ?",
        )
        .bind(task_id.as_uuid().to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load task sync link", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble_row(&row)?))
    }

    pub(super) async fn load_by_issue(
        pool: &SqlitePool,
        project_id: ProjectId,
        external_issue_number: u64,
    ) -> Result<Option<TaskSyncLink>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT task_id, project_id, external_issue_number, external_url, \
             last_remote_updated_at, last_local_synced_at, last_comment_synced_at, created_at \
             FROM task_sync_links WHERE project_id = ? AND external_issue_number = ?",
        )
        .bind(project_id.as_uuid().to_string())
        .bind(i64::try_from(external_issue_number).unwrap_or(i64::MAX))
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load task sync link", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble_row(&row)?))
    }

    pub(super) async fn list_for_project(
        pool: &SqlitePool,
        project_id: ProjectId,
    ) -> Result<Vec<TaskSyncLink>, RepoError> {
        let rows = sqlx::query(
            "SELECT task_id, project_id, external_issue_number, external_url, \
             last_remote_updated_at, last_local_synced_at, last_comment_synced_at, created_at \
             FROM task_sync_links WHERE project_id = ?",
        )
        .bind(project_id.as_uuid().to_string())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list task sync links", e))?;
        rows.iter().map(assemble_row).collect()
    }

    pub(super) async fn insert(pool: &SqlitePool, link: &TaskSyncLink) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO task_sync_links \
             (task_id, project_id, external_issue_number, external_url, \
              last_remote_updated_at, last_local_synced_at, last_comment_synced_at, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(link.task_id.as_uuid().to_string())
        .bind(link.project_id.as_uuid().to_string())
        .bind(i64::try_from(link.external_issue_number).unwrap_or(i64::MAX))
        .bind(&link.external_url)
        .bind(link.last_remote_updated_at.unix_seconds())
        .bind(link.last_local_synced_at.unix_seconds())
        .bind(link.last_comment_synced_at.map(|t| t.unix_seconds()))
        .bind(link.created_at.unix_seconds())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert task sync link", e))?;
        Ok(())
    }

    pub(super) async fn update(pool: &SqlitePool, link: &TaskSyncLink) -> Result<(), RepoError> {
        sqlx::query(
            "UPDATE task_sync_links SET external_url = ?, last_remote_updated_at = ?, \
             last_local_synced_at = ?, last_comment_synced_at = ? WHERE task_id = ?",
        )
        .bind(&link.external_url)
        .bind(link.last_remote_updated_at.unix_seconds())
        .bind(link.last_local_synced_at.unix_seconds())
        .bind(link.last_comment_synced_at.map(|t| t.unix_seconds()))
        .bind(link.task_id.as_uuid().to_string())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to update task sync link", e))?;
        Ok(())
    }
}

mod postgres_impl {
    use super::*;

    fn assemble_row(row: &sqlx::postgres::PgRow) -> Result<TaskSyncLink, RepoError> {
        assemble(
            row.get::<uuid::Uuid, _>("task_id"),
            row.get::<uuid::Uuid, _>("project_id"),
            row.get::<i64, _>("external_issue_number"),
            row.get("external_url"),
            row.get::<i64, _>("last_remote_updated_at"),
            row.get::<i64, _>("last_local_synced_at"),
            row.get::<Option<i64>, _>("last_comment_synced_at"),
            row.get::<i64, _>("created_at"),
        )
    }

    pub(super) async fn load_by_task(
        pool: &PgPool,
        task_id: TaskId,
    ) -> Result<Option<TaskSyncLink>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT task_id, project_id, external_issue_number, external_url, \
             last_remote_updated_at, last_local_synced_at, last_comment_synced_at, created_at \
             FROM task_sync_links WHERE task_id = $1",
        )
        .bind(task_id.as_uuid())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load task sync link", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble_row(&row)?))
    }

    pub(super) async fn load_by_issue(
        pool: &PgPool,
        project_id: ProjectId,
        external_issue_number: u64,
    ) -> Result<Option<TaskSyncLink>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT task_id, project_id, external_issue_number, external_url, \
             last_remote_updated_at, last_local_synced_at, last_comment_synced_at, created_at \
             FROM task_sync_links WHERE project_id = $1 AND external_issue_number = $2",
        )
        .bind(project_id.as_uuid())
        .bind(i64::try_from(external_issue_number).unwrap_or(i64::MAX))
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load task sync link", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble_row(&row)?))
    }

    pub(super) async fn list_for_project(
        pool: &PgPool,
        project_id: ProjectId,
    ) -> Result<Vec<TaskSyncLink>, RepoError> {
        let rows = sqlx::query(
            "SELECT task_id, project_id, external_issue_number, external_url, \
             last_remote_updated_at, last_local_synced_at, last_comment_synced_at, created_at \
             FROM task_sync_links WHERE project_id = $1",
        )
        .bind(project_id.as_uuid())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list task sync links", e))?;
        rows.iter().map(assemble_row).collect()
    }

    pub(super) async fn insert(pool: &PgPool, link: &TaskSyncLink) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO task_sync_links \
             (task_id, project_id, external_issue_number, external_url, \
              last_remote_updated_at, last_local_synced_at, last_comment_synced_at, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(link.task_id.as_uuid())
        .bind(link.project_id.as_uuid())
        .bind(i64::try_from(link.external_issue_number).unwrap_or(i64::MAX))
        .bind(&link.external_url)
        .bind(link.last_remote_updated_at.unix_seconds())
        .bind(link.last_local_synced_at.unix_seconds())
        .bind(link.last_comment_synced_at.map(|t| t.unix_seconds()))
        .bind(link.created_at.unix_seconds())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert task sync link", e))?;
        Ok(())
    }

    pub(super) async fn update(pool: &PgPool, link: &TaskSyncLink) -> Result<(), RepoError> {
        sqlx::query(
            "UPDATE task_sync_links SET external_url = $1, last_remote_updated_at = $2, \
             last_local_synced_at = $3, last_comment_synced_at = $4 WHERE task_id = $5",
        )
        .bind(&link.external_url)
        .bind(link.last_remote_updated_at.unix_seconds())
        .bind(link.last_local_synced_at.unix_seconds())
        .bind(link.last_comment_synced_at.map(|t| t.unix_seconds()))
        .bind(link.task_id.as_uuid())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to update task sync link", e))?;
        Ok(())
    }
}

#[async_trait]
impl TaskSyncLinkRepository for SqlStore {
    async fn load_by_task(&self, task_id: TaskId) -> Result<Option<TaskSyncLink>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::load_by_task(pool, task_id).await,
            Backend::Postgres(pool) => postgres_impl::load_by_task(pool, task_id).await,
        }
    }

    async fn load_by_issue(
        &self,
        project_id: ProjectId,
        external_issue_number: u64,
    ) -> Result<Option<TaskSyncLink>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlite_impl::load_by_issue(pool, project_id, external_issue_number).await
            }
            Backend::Postgres(pool) => {
                postgres_impl::load_by_issue(pool, project_id, external_issue_number).await
            }
        }
    }

    async fn list_for_project(&self, project_id: ProjectId) -> Result<Vec<TaskSyncLink>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_for_project(pool, project_id).await,
            Backend::Postgres(pool) => postgres_impl::list_for_project(pool, project_id).await,
        }
    }

    async fn insert(&self, link: &TaskSyncLink) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::insert(pool, link).await,
            Backend::Postgres(pool) => postgres_impl::insert(pool, link).await,
        }
    }

    async fn update(&self, link: &TaskSyncLink) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::update(pool, link).await,
            Backend::Postgres(pool) => postgres_impl::update(pool, link).await,
        }
    }
}
