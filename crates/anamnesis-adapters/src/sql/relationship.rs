//! [`RelationshipRepository`] over [`SqlStore`]: standalone edges, living
//! outside any project (`docs/DOMAIN.md` §3) — no project-scoping parameter
//! anywhere in this module.

use anamnesis_app::{RelationshipRepository, RepoError};
use anamnesis_core::{KindId, Relationship, RelationshipId, TaskId};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::{Backend, SqlStore, parse_uuid, timestamp_from_seconds};

fn assemble(
    id: uuid::Uuid,
    from_task_id: uuid::Uuid,
    to_task_id: uuid::Uuid,
    kind_id: uuid::Uuid,
    created_at: i64,
) -> Result<Relationship, RepoError> {
    Ok(Relationship {
        id: RelationshipId::new(id),
        from_task_id: TaskId::new(from_task_id),
        to_task_id: TaskId::new(to_task_id),
        kind_id: KindId::new(kind_id),
        created_at: timestamp_from_seconds(created_at)?,
    })
}

const BUILTIN_BLOCKS_TEXT: &str = "a1a50000-0000-0000-0000-000000000001";

mod sqlite_impl {
    use super::*;

    pub(super) async fn load(
        pool: &SqlitePool,
        id: RelationshipId,
    ) -> Result<Option<Relationship>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT id, from_task_id, to_task_id, kind_id, created_at \
             FROM relationships WHERE id = ?",
        )
        .bind(id.as_uuid().to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load relationship", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble(
            parse_uuid(&row.get::<String, _>("id"))?,
            parse_uuid(&row.get::<String, _>("from_task_id"))?,
            parse_uuid(&row.get::<String, _>("to_task_id"))?,
            parse_uuid(&row.get::<String, _>("kind_id"))?,
            row.get::<i64, _>("created_at"),
        )?))
    }

    pub(super) async fn list_for_task(
        pool: &SqlitePool,
        task_id: TaskId,
    ) -> Result<Vec<Relationship>, RepoError> {
        let id_text = task_id.as_uuid().to_string();
        let rows = sqlx::query(
            "SELECT id, from_task_id, to_task_id, kind_id, created_at FROM relationships \
             WHERE from_task_id = ? OR to_task_id = ? ORDER BY created_at",
        )
        .bind(&id_text)
        .bind(&id_text)
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list relationships for task", e))?;
        rows.into_iter()
            .map(|row| {
                assemble(
                    parse_uuid(&row.get::<String, _>("id"))?,
                    parse_uuid(&row.get::<String, _>("from_task_id"))?,
                    parse_uuid(&row.get::<String, _>("to_task_id"))?,
                    parse_uuid(&row.get::<String, _>("kind_id"))?,
                    row.get::<i64, _>("created_at"),
                )
            })
            .collect()
    }

    pub(super) async fn list_blocking(pool: &SqlitePool) -> Result<Vec<Relationship>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, from_task_id, to_task_id, kind_id, created_at FROM relationships \
             WHERE kind_id = ? ORDER BY created_at",
        )
        .bind(BUILTIN_BLOCKS_TEXT)
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list blocking relationships", e))?;
        rows.into_iter()
            .map(|row| {
                assemble(
                    parse_uuid(&row.get::<String, _>("id"))?,
                    parse_uuid(&row.get::<String, _>("from_task_id"))?,
                    parse_uuid(&row.get::<String, _>("to_task_id"))?,
                    parse_uuid(&row.get::<String, _>("kind_id"))?,
                    row.get::<i64, _>("created_at"),
                )
            })
            .collect()
    }

    pub(super) async fn insert(
        pool: &SqlitePool,
        relationship: &Relationship,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO relationships (id, from_task_id, to_task_id, kind_id, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(relationship.id.as_uuid().to_string())
        .bind(relationship.from_task_id.as_uuid().to_string())
        .bind(relationship.to_task_id.as_uuid().to_string())
        .bind(relationship.kind_id.as_uuid().to_string())
        .bind(relationship.created_at.unix_seconds())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert relationship", e))?;
        Ok(())
    }

    pub(super) async fn delete(pool: &SqlitePool, id: RelationshipId) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM relationships WHERE id = ?")
            .bind(id.as_uuid().to_string())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to delete relationship", e))?;
        Ok(())
    }
}

mod postgres_impl {
    use super::*;

    pub(super) async fn load(
        pool: &PgPool,
        id: RelationshipId,
    ) -> Result<Option<Relationship>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT id, from_task_id, to_task_id, kind_id, created_at \
             FROM relationships WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load relationship", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble(
            row.get::<uuid::Uuid, _>("id"),
            row.get::<uuid::Uuid, _>("from_task_id"),
            row.get::<uuid::Uuid, _>("to_task_id"),
            row.get::<uuid::Uuid, _>("kind_id"),
            row.get::<i64, _>("created_at"),
        )?))
    }

    pub(super) async fn list_for_task(
        pool: &PgPool,
        task_id: TaskId,
    ) -> Result<Vec<Relationship>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, from_task_id, to_task_id, kind_id, created_at FROM relationships \
             WHERE from_task_id = $1 OR to_task_id = $1 ORDER BY created_at",
        )
        .bind(task_id.as_uuid())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list relationships for task", e))?;
        rows.into_iter()
            .map(|row| {
                assemble(
                    row.get::<uuid::Uuid, _>("id"),
                    row.get::<uuid::Uuid, _>("from_task_id"),
                    row.get::<uuid::Uuid, _>("to_task_id"),
                    row.get::<uuid::Uuid, _>("kind_id"),
                    row.get::<i64, _>("created_at"),
                )
            })
            .collect()
    }

    pub(super) async fn list_blocking(pool: &PgPool) -> Result<Vec<Relationship>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, from_task_id, to_task_id, kind_id, created_at FROM relationships \
             WHERE kind_id = $1 ORDER BY created_at",
        )
        .bind(KindId::BUILTIN_BLOCKS.as_uuid())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list blocking relationships", e))?;
        rows.into_iter()
            .map(|row| {
                assemble(
                    row.get::<uuid::Uuid, _>("id"),
                    row.get::<uuid::Uuid, _>("from_task_id"),
                    row.get::<uuid::Uuid, _>("to_task_id"),
                    row.get::<uuid::Uuid, _>("kind_id"),
                    row.get::<i64, _>("created_at"),
                )
            })
            .collect()
    }

    pub(super) async fn insert(
        pool: &PgPool,
        relationship: &Relationship,
    ) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO relationships (id, from_task_id, to_task_id, kind_id, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(relationship.id.as_uuid())
        .bind(relationship.from_task_id.as_uuid())
        .bind(relationship.to_task_id.as_uuid())
        .bind(relationship.kind_id.as_uuid())
        .bind(relationship.created_at.unix_seconds())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert relationship", e))?;
        Ok(())
    }

    pub(super) async fn delete(pool: &PgPool, id: RelationshipId) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM relationships WHERE id = $1")
            .bind(id.as_uuid())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to delete relationship", e))?;
        Ok(())
    }
}

#[async_trait]
impl RelationshipRepository for SqlStore {
    async fn load(&self, id: RelationshipId) -> Result<Option<Relationship>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::load(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::load(pool, id).await,
        }
    }

    async fn list_for_task(&self, task_id: TaskId) -> Result<Vec<Relationship>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_for_task(pool, task_id).await,
            Backend::Postgres(pool) => postgres_impl::list_for_task(pool, task_id).await,
        }
    }

    async fn list_blocking(&self) -> Result<Vec<Relationship>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_blocking(pool).await,
            Backend::Postgres(pool) => postgres_impl::list_blocking(pool).await,
        }
    }

    async fn insert(&self, relationship: &Relationship) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::insert(pool, relationship).await,
            Backend::Postgres(pool) => postgres_impl::insert(pool, relationship).await,
        }
    }

    async fn delete(&self, id: RelationshipId) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::delete(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::delete(pool, id).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `BUILTIN_BLOCKS_TEXT` is a hand-written literal (SQLite has no native
    /// UUID type to bind `KindId::BUILTIN_BLOCKS` directly, unlike the
    /// Postgres path just above); this pins it to the real constant so the
    /// two can never silently drift apart.
    #[test]
    fn builtin_blocks_text_matches_the_real_constant() {
        assert_eq!(
            BUILTIN_BLOCKS_TEXT,
            KindId::BUILTIN_BLOCKS.as_uuid().to_string()
        );
    }
}
