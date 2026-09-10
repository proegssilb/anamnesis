//! [`CommentRepository`] over [`SqlStore`]: paged per task, append-heavy
//! (`docs/DOMAIN.md` §3). Four nullable columns (`external_*`) carry an
//! imported comment's origin (issues #40/#41) — see
//! `anamnesis_app::entities`'s module doc comment for why that's columns on
//! this table rather than a separate one.

use anamnesis_app::{Comment, CommentId, CommentOrigin, CommentRepository, RepoError};
use anamnesis_core::{TaskId, UserId};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::{
    Backend, SqlStore, parse_uuid, sync_provider_from_text, sync_provider_to_text,
    timestamp_from_seconds,
};

/// Reconstructs a `CommentOrigin` from its four stored columns — `Some`
/// only when `external_comment_id` is non-null (all four travel together,
/// see this module's doc comment).
fn assemble_origin(
    external_provider: Option<String>,
    external_comment_id: Option<i64>,
    external_url: Option<String>,
    external_author_display: Option<String>,
) -> Result<Option<CommentOrigin>, RepoError> {
    let Some(external_comment_id) = external_comment_id else {
        return Ok(None);
    };
    let provider = external_provider
        .as_deref()
        .ok_or_else(|| RepoError::new("comment has an external_comment_id but no provider"))?;
    let external_url = external_url
        .ok_or_else(|| RepoError::new("comment has an external_comment_id but no external_url"))?;
    let external_author_display = external_author_display.ok_or_else(|| {
        RepoError::new("comment has an external_comment_id but no external_author_display")
    })?;
    Ok(Some(CommentOrigin {
        provider: sync_provider_from_text(provider)?,
        external_comment_id: u64::try_from(external_comment_id)
            .map_err(|e| RepoError::from_source("invalid stored external comment id", e))?,
        external_url,
        external_author_display,
    }))
}

#[allow(clippy::too_many_arguments)]
fn assemble(
    id: uuid::Uuid,
    task_id: uuid::Uuid,
    author: String,
    body: String,
    created_at: i64,
    edited_at: Option<i64>,
    external_provider: Option<String>,
    external_comment_id: Option<i64>,
    external_url: Option<String>,
    external_author_display: Option<String>,
) -> Result<Comment, RepoError> {
    Ok(Comment {
        id: CommentId::new(id),
        task_id: TaskId::new(task_id),
        author: UserId::new(author),
        body,
        created_at: timestamp_from_seconds(created_at)?,
        edited_at: edited_at.map(timestamp_from_seconds).transpose()?,
        origin: assemble_origin(
            external_provider,
            external_comment_id,
            external_url,
            external_author_display,
        )?,
    })
}

mod sqlite_impl {
    use super::*;

    fn assemble_row(row: &sqlx::sqlite::SqliteRow) -> Result<Comment, RepoError> {
        assemble(
            parse_uuid(&row.get::<String, _>("id"))?,
            parse_uuid(&row.get::<String, _>("task_id"))?,
            row.get("author"),
            row.get("body"),
            row.get::<i64, _>("created_at"),
            row.get::<Option<i64>, _>("edited_at"),
            row.get("external_provider"),
            row.get::<Option<i64>, _>("external_comment_id"),
            row.get("external_url"),
            row.get("external_author_display"),
        )
    }

    pub(super) async fn list_for_task(
        pool: &SqlitePool,
        task_id: TaskId,
    ) -> Result<Vec<Comment>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, task_id, author, body, created_at, edited_at, external_provider, \
             external_comment_id, external_url, external_author_display \
             FROM comments WHERE task_id = ? ORDER BY created_at",
        )
        .bind(task_id.as_uuid().to_string())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list comments for task", e))?;
        rows.iter().map(assemble_row).collect()
    }

    pub(super) async fn load(
        pool: &SqlitePool,
        id: CommentId,
    ) -> Result<Option<Comment>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT id, task_id, author, body, created_at, edited_at, external_provider, \
             external_comment_id, external_url, external_author_display \
             FROM comments WHERE id = ?",
        )
        .bind(id.as_uuid().to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load comment", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble_row(&row)?))
    }

    pub(super) async fn insert(pool: &SqlitePool, comment: &Comment) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO comments \
             (id, task_id, author, body, created_at, edited_at, \
              external_provider, external_comment_id, external_url, external_author_display) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(comment.id.as_uuid().to_string())
        .bind(comment.task_id.as_uuid().to_string())
        .bind(comment.author.as_str())
        .bind(&comment.body)
        .bind(comment.created_at.unix_seconds())
        .bind(comment.edited_at.map(|t| t.unix_seconds()))
        .bind(
            comment
                .origin
                .as_ref()
                .map(|o| sync_provider_to_text(o.provider)),
        )
        .bind(
            comment
                .origin
                .as_ref()
                .map(|o| i64::try_from(o.external_comment_id).unwrap_or(i64::MAX)),
        )
        .bind(comment.origin.as_ref().map(|o| o.external_url.clone()))
        .bind(
            comment
                .origin
                .as_ref()
                .map(|o| o.external_author_display.clone()),
        )
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert comment", e))?;
        Ok(())
    }

    pub(super) async fn update(pool: &SqlitePool, comment: &Comment) -> Result<(), RepoError> {
        sqlx::query("UPDATE comments SET body = ?, edited_at = ? WHERE id = ?")
            .bind(&comment.body)
            .bind(comment.edited_at.map(|t| t.unix_seconds()))
            .bind(comment.id.as_uuid().to_string())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to update comment", e))?;
        Ok(())
    }

    pub(super) async fn delete(pool: &SqlitePool, id: CommentId) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM comments WHERE id = ?")
            .bind(id.as_uuid().to_string())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to delete comment", e))?;
        Ok(())
    }

    pub(super) async fn exists_with_external_comment_id(
        pool: &SqlitePool,
        task_id: TaskId,
        external_comment_id: u64,
    ) -> Result<bool, RepoError> {
        let row = sqlx::query(
            "SELECT EXISTS(SELECT 1 FROM comments WHERE task_id = ? AND external_comment_id = ?) AS found",
        )
        .bind(task_id.as_uuid().to_string())
        .bind(i64::try_from(external_comment_id).unwrap_or(i64::MAX))
        .fetch_one(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to check for an imported comment", e))?;
        Ok(row.get::<i64, _>("found") != 0)
    }
}

mod postgres_impl {
    use super::*;

    fn assemble_row(row: &sqlx::postgres::PgRow) -> Result<Comment, RepoError> {
        assemble(
            row.get::<uuid::Uuid, _>("id"),
            row.get::<uuid::Uuid, _>("task_id"),
            row.get("author"),
            row.get("body"),
            row.get::<i64, _>("created_at"),
            row.get::<Option<i64>, _>("edited_at"),
            row.get("external_provider"),
            row.get::<Option<i64>, _>("external_comment_id"),
            row.get("external_url"),
            row.get("external_author_display"),
        )
    }

    pub(super) async fn list_for_task(
        pool: &PgPool,
        task_id: TaskId,
    ) -> Result<Vec<Comment>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, task_id, author, body, created_at, edited_at, external_provider, \
             external_comment_id, external_url, external_author_display \
             FROM comments WHERE task_id = $1 ORDER BY created_at",
        )
        .bind(task_id.as_uuid())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list comments for task", e))?;
        rows.iter().map(assemble_row).collect()
    }

    pub(super) async fn load(pool: &PgPool, id: CommentId) -> Result<Option<Comment>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT id, task_id, author, body, created_at, edited_at, external_provider, \
             external_comment_id, external_url, external_author_display \
             FROM comments WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load comment", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble_row(&row)?))
    }

    pub(super) async fn insert(pool: &PgPool, comment: &Comment) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO comments \
             (id, task_id, author, body, created_at, edited_at, \
              external_provider, external_comment_id, external_url, external_author_display) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(comment.id.as_uuid())
        .bind(comment.task_id.as_uuid())
        .bind(comment.author.as_str())
        .bind(&comment.body)
        .bind(comment.created_at.unix_seconds())
        .bind(comment.edited_at.map(|t| t.unix_seconds()))
        .bind(
            comment
                .origin
                .as_ref()
                .map(|o| sync_provider_to_text(o.provider)),
        )
        .bind(
            comment
                .origin
                .as_ref()
                .map(|o| i64::try_from(o.external_comment_id).unwrap_or(i64::MAX)),
        )
        .bind(comment.origin.as_ref().map(|o| o.external_url.clone()))
        .bind(
            comment
                .origin
                .as_ref()
                .map(|o| o.external_author_display.clone()),
        )
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert comment", e))?;
        Ok(())
    }

    pub(super) async fn update(pool: &PgPool, comment: &Comment) -> Result<(), RepoError> {
        sqlx::query("UPDATE comments SET body = $1, edited_at = $2 WHERE id = $3")
            .bind(&comment.body)
            .bind(comment.edited_at.map(|t| t.unix_seconds()))
            .bind(comment.id.as_uuid())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to update comment", e))?;
        Ok(())
    }

    pub(super) async fn delete(pool: &PgPool, id: CommentId) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM comments WHERE id = $1")
            .bind(id.as_uuid())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to delete comment", e))?;
        Ok(())
    }

    pub(super) async fn exists_with_external_comment_id(
        pool: &PgPool,
        task_id: TaskId,
        external_comment_id: u64,
    ) -> Result<bool, RepoError> {
        let row = sqlx::query(
            "SELECT EXISTS(SELECT 1 FROM comments WHERE task_id = $1 AND external_comment_id = $2) AS found",
        )
        .bind(task_id.as_uuid())
        .bind(i64::try_from(external_comment_id).unwrap_or(i64::MAX))
        .fetch_one(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to check for an imported comment", e))?;
        Ok(row.get("found"))
    }
}

#[async_trait]
impl CommentRepository for SqlStore {
    async fn list_for_task(&self, task_id: TaskId) -> Result<Vec<Comment>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_for_task(pool, task_id).await,
            Backend::Postgres(pool) => postgres_impl::list_for_task(pool, task_id).await,
        }
    }

    async fn load(&self, id: CommentId) -> Result<Option<Comment>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::load(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::load(pool, id).await,
        }
    }

    async fn insert(&self, comment: &Comment) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::insert(pool, comment).await,
            Backend::Postgres(pool) => postgres_impl::insert(pool, comment).await,
        }
    }

    async fn update(&self, comment: &Comment) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::update(pool, comment).await,
            Backend::Postgres(pool) => postgres_impl::update(pool, comment).await,
        }
    }

    async fn delete(&self, id: CommentId) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::delete(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::delete(pool, id).await,
        }
    }

    async fn exists_with_external_comment_id(
        &self,
        task_id: TaskId,
        external_comment_id: u64,
    ) -> Result<bool, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlite_impl::exists_with_external_comment_id(pool, task_id, external_comment_id)
                    .await
            }
            Backend::Postgres(pool) => {
                postgres_impl::exists_with_external_comment_id(pool, task_id, external_comment_id)
                    .await
            }
        }
    }
}
