//! [`AreaRepository`] over [`SqlStore`]: tiny aggregate, no children.

use anamnesis_app::{AreaRepository, RepoError};
use anamnesis_core::{Area, AreaId};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::{Backend, SqlStore, parse_uuid, timestamp_from_seconds, title_from_text};

fn assemble(
    id: uuid::Uuid,
    title: String,
    description: String,
    position: i64,
    created_at: i64,
    updated_at: i64,
) -> Result<Area, RepoError> {
    Ok(Area {
        id: AreaId::new(id),
        title: title_from_text(title)?,
        description,
        position: u32::try_from(position)
            .map_err(|e| RepoError::from_source("invalid stored area position", e))?,
        created_at: timestamp_from_seconds(created_at)?,
        updated_at: timestamp_from_seconds(updated_at)?,
    })
}

mod sqlite_impl {
    use super::*;

    pub(super) async fn load(pool: &SqlitePool, id: AreaId) -> Result<Option<Area>, RepoError> {
        let id_text = id.as_uuid().to_string();
        let Some(row) = sqlx::query(
            "SELECT id, title, description, position, created_at, updated_at \
             FROM areas WHERE id = ?",
        )
        .bind(&id_text)
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load area", e))?
        else {
            return Ok(None);
        };
        let raw_id: String = row.get("id");
        Ok(Some(assemble(
            parse_uuid(&raw_id)?,
            row.get("title"),
            row.get("description"),
            row.get::<i64, _>("position"),
            row.get::<i64, _>("created_at"),
            row.get::<i64, _>("updated_at"),
        )?))
    }

    pub(super) async fn list(pool: &SqlitePool) -> Result<Vec<Area>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, title, description, position, created_at, updated_at \
             FROM areas ORDER BY position",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list areas", e))?;
        rows.into_iter()
            .map(|row| {
                let raw_id: String = row.get("id");
                assemble(
                    parse_uuid(&raw_id)?,
                    row.get("title"),
                    row.get("description"),
                    row.get::<i64, _>("position"),
                    row.get::<i64, _>("created_at"),
                    row.get::<i64, _>("updated_at"),
                )
            })
            .collect()
    }

    pub(super) async fn insert(pool: &SqlitePool, area: &Area) -> Result<(), RepoError> {
        sqlx::query(
            "INSERT INTO areas (id, title, description, position, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(area.id.as_uuid().to_string())
        .bind(area.title.as_str())
        .bind(&area.description)
        .bind(i64::from(area.position))
        .bind(area.created_at.unix_seconds())
        .bind(area.updated_at.unix_seconds())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert area", e))?;
        Ok(())
    }

    pub(super) async fn update(pool: &SqlitePool, area: &Area) -> Result<(), RepoError> {
        sqlx::query(
            "UPDATE areas SET title = ?, description = ?, position = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(area.title.as_str())
        .bind(&area.description)
        .bind(i64::from(area.position))
        .bind(area.updated_at.unix_seconds())
        .bind(area.id.as_uuid().to_string())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to update area", e))?;
        Ok(())
    }
}

mod postgres_impl {
    use super::*;

    pub(super) async fn load(pool: &PgPool, id: AreaId) -> Result<Option<Area>, RepoError> {
        let Some(row) = sqlx::query(
            "SELECT id, title, description, position, created_at, updated_at \
             FROM areas WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load area", e))?
        else {
            return Ok(None);
        };
        Ok(Some(assemble(
            row.get::<uuid::Uuid, _>("id"),
            row.get("title"),
            row.get("description"),
            i64::from(row.get::<i32, _>("position")),
            row.get::<i64, _>("created_at"),
            row.get::<i64, _>("updated_at"),
        )?))
    }

    pub(super) async fn list(pool: &PgPool) -> Result<Vec<Area>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, title, description, position, created_at, updated_at \
             FROM areas ORDER BY position",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list areas", e))?;
        rows.into_iter()
            .map(|row| {
                assemble(
                    row.get::<uuid::Uuid, _>("id"),
                    row.get("title"),
                    row.get("description"),
                    i64::from(row.get::<i32, _>("position")),
                    row.get::<i64, _>("created_at"),
                    row.get::<i64, _>("updated_at"),
                )
            })
            .collect()
    }

    pub(super) async fn insert(pool: &PgPool, area: &Area) -> Result<(), RepoError> {
        let position = i32::try_from(area.position)
            .map_err(|e| RepoError::from_source("area position out of range", e))?;
        sqlx::query(
            "INSERT INTO areas (id, title, description, position, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(area.id.as_uuid())
        .bind(area.title.as_str())
        .bind(&area.description)
        .bind(position)
        .bind(area.created_at.unix_seconds())
        .bind(area.updated_at.unix_seconds())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert area", e))?;
        Ok(())
    }

    pub(super) async fn update(pool: &PgPool, area: &Area) -> Result<(), RepoError> {
        let position = i32::try_from(area.position)
            .map_err(|e| RepoError::from_source("area position out of range", e))?;
        sqlx::query(
            "UPDATE areas SET title = $1, description = $2, position = $3, updated_at = $4 \
             WHERE id = $5",
        )
        .bind(area.title.as_str())
        .bind(&area.description)
        .bind(position)
        .bind(area.updated_at.unix_seconds())
        .bind(area.id.as_uuid())
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to update area", e))?;
        Ok(())
    }
}

#[async_trait]
impl AreaRepository for SqlStore {
    async fn load(&self, id: AreaId) -> Result<Option<Area>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::load(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::load(pool, id).await,
        }
    }

    async fn list(&self) -> Result<Vec<Area>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list(pool).await,
            Backend::Postgres(pool) => postgres_impl::list(pool).await,
        }
    }

    async fn insert(&self, area: &Area) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::insert(pool, area).await,
            Backend::Postgres(pool) => postgres_impl::insert(pool, area).await,
        }
    }

    async fn update(&self, area: &Area) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::update(pool, area).await,
            Backend::Postgres(pool) => postgres_impl::update(pool, area).await,
        }
    }
}
