//! [`CommentRepository`] over [`SqlStore`]: paged per task, append-heavy
//! (`docs/DOMAIN.md` §3).

use anamnesis_app::{Comment, CommentId, CommentRepository, RepoError};
use anamnesis_core::{TaskId, UserId};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::{Backend, SqlStore, parse_uuid, timestamp_from_seconds};

fn assemble(
    id: uuid::Uuid,
    task_id: uuid::Uuid,
    author: String,
    body: String,
    created_at: i64,
    edited_at: Option<i64>,
) -> Result<Comment, RepoError> {
    Ok(Comment {
        id: CommentId::new(id),
        task_id: TaskId::new(task_id),
        author: UserId::new(author),
        body,
        created_at: timestamp_from_seconds(created_at)?,
        edited_at: edited_at.map(timestamp_from_seconds).transpose()?,
    })
}

mod sqlite_impl {
    use super::*;

    pub(super) async fn list_for_task(
        pool: &SqlitePool,
        task_id: TaskId,
    ) -> Result<Vec<Comment>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, task_id, author, body, created_at, edited_at FROM comments \
             WHERE task_id = ? ORDER BY created_at",
        )
        .bind(task_id.as_uuid().to_string())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list comments for task", e))?;
        rows.into_iter()
            .map(|row| {
                assemble(
                    parse_uuid(&row.get::<String, _>("id"))?,
                    parse_uuid(&row.get::<String, _>("task_id"))?,
                    row.get("author"),
                    row.get("body"),
                    row.get::<i64, _>("created_at"),
                    row.get::<Option<i64>, _>("edited_at"),
                )
            })
            .collect()
    }

    pub(super) async fn load(
        pool: &SqlitePool,
        id: CommentId,
    ) -> Result<Option<Comment>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT id, task_id, author, body, created_at, edited_at FROM comments WHERE id = ?",
        )
        .bind(id.as_uuid().to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load comment", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble(
            parse_uuid(&row.get::<String, _>("id"))?,
            parse_uuid(&row.get::<String, _>("task_id"))?,
            row.get("author"),
            row.get("body"),
            row.get::<i64, _>("created_at"),
            row.get::<Option<i64>, _>("edited_at"),
        )?))
    }

    pub(super) async fn insert(pool: &SqlitePool, comment: &Comment) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO comments (id, task_id, author, body, created_at, edited_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(comment.id.as_uuid().to_string())
        .bind(comment.task_id.as_uuid().to_string())
        .bind(comment.author.as_str())
        .bind(&comment.body)
        .bind(comment.created_at.unix_seconds())
        .bind(comment.edited_at.map(|t| t.unix_seconds()))
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
}

mod postgres_impl {
    use super::*;

    pub(super) async fn list_for_task(
        pool: &PgPool,
        task_id: TaskId,
    ) -> Result<Vec<Comment>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, task_id, author, body, created_at, edited_at FROM comments \
             WHERE task_id = $1 ORDER BY created_at",
        )
        .bind(task_id.as_uuid())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list comments for task", e))?;
        rows.into_iter()
            .map(|row| {
                assemble(
                    row.get::<uuid::Uuid, _>("id"),
                    row.get::<uuid::Uuid, _>("task_id"),
                    row.get("author"),
                    row.get("body"),
                    row.get::<i64, _>("created_at"),
                    row.get::<Option<i64>, _>("edited_at"),
                )
            })
            .collect()
    }

    pub(super) async fn load(pool: &PgPool, id: CommentId) -> Result<Option<Comment>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT id, task_id, author, body, created_at, edited_at FROM comments WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load comment", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble(
            row.get::<uuid::Uuid, _>("id"),
            row.get::<uuid::Uuid, _>("task_id"),
            row.get("author"),
            row.get("body"),
            row.get::<i64, _>("created_at"),
            row.get::<Option<i64>, _>("edited_at"),
        )?))
    }

    pub(super) async fn insert(pool: &PgPool, comment: &Comment) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO comments (id, task_id, author, body, created_at, edited_at) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(comment.id.as_uuid())
        .bind(comment.task_id.as_uuid())
        .bind(comment.author.as_str())
        .bind(&comment.body)
        .bind(comment.created_at.unix_seconds())
        .bind(comment.edited_at.map(|t| t.unix_seconds()))
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
}
