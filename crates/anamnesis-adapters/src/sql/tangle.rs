//! [`TangleRepository`] over [`SqlStore`]: system-derived, reconciled
//! against fresh detection passes (`docs/DOMAIN.md` §3).
//!
//! `Tangle::fingerprint` is never stored: `anamnesis_core::Fingerprint`
//! exposes no accessor to its inner value, only the pure constructor
//! [`anamnesis_core::Fingerprint::of`] — so it is recomputed from the
//! loaded `task_ids` on every read instead, which is exactly what
//! `Fingerprint::of` is for and costs nothing extra to keep correct.

use std::collections::BTreeSet;

use anamnesis_app::{RepoError, TangleRepository};
use anamnesis_core::{Fingerprint, Tangle, TangleId, TaskId};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::{Backend, SqlStore, parse_uuid, timestamp_from_seconds};

fn assemble(
    id: uuid::Uuid,
    task_ids: BTreeSet<TaskId>,
    detected_at: i64,
    resolved_at: Option<i64>,
) -> Result<Tangle, RepoError> {
    Ok(Tangle {
        id: TangleId::new(id),
        fingerprint: Fingerprint::of(&task_ids),
        task_ids,
        detected_at: timestamp_from_seconds(detected_at)?,
        resolved_at: resolved_at.map(timestamp_from_seconds).transpose()?,
    })
}

mod sqlite_impl {
    use super::*;

    pub(super) async fn list_active(pool: &SqlitePool) -> Result<Vec<Tangle>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, detected_at, resolved_at FROM tangles WHERE resolved_at IS NULL",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list active tangles", e))?;
        let mut tangles = Vec::with_capacity(rows.len());
        for row in rows {
            let id_text: String = row.get("id");
            let id = parse_uuid(&id_text)?;
            let task_rows = sqlx::query("SELECT task_id FROM tangle_tasks WHERE tangle_id = ?")
                .bind(&id_text)
                .fetch_all(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to load tangle tasks", e))?;
            let task_ids = task_rows
                .into_iter()
                .map(|r| Ok(TaskId::new(parse_uuid(&r.get::<String, _>("task_id"))?)))
                .collect::<Result<BTreeSet<_>, RepoError>>()?;
            tangles.push(assemble(
                id,
                task_ids,
                row.get::<i64, _>("detected_at"),
                row.get::<Option<i64>, _>("resolved_at"),
            )?);
        }
        Ok(tangles)
    }

    pub(super) async fn insert(pool: &SqlitePool, tangle: &Tangle) -> Result<(), RepoError> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| RepoError::from_source("failed to start transaction", e))?;
        sqlx::query("INSERT INTO tangles (id, detected_at, resolved_at) VALUES (?, ?, ?)")
            .bind(tangle.id.as_uuid().to_string())
            .bind(tangle.detected_at.unix_seconds())
            .bind(tangle.resolved_at.map(|t| t.unix_seconds()))
            .execute(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to insert tangle", e))?;
        for task_id in &tangle.task_ids {
            sqlx::query("INSERT INTO tangle_tasks (tangle_id, task_id) VALUES (?, ?)")
                .bind(tangle.id.as_uuid().to_string())
                .bind(task_id.as_uuid().to_string())
                .execute(&mut *tx)
                .await
                .map_err(|e| RepoError::from_source("failed to insert tangle task", e))?;
        }
        tx.commit()
            .await
            .map_err(|e| RepoError::from_source("failed to commit tangle insert", e))
    }

    pub(super) async fn update(pool: &SqlitePool, tangle: &Tangle) -> Result<(), RepoError> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| RepoError::from_source("failed to start transaction", e))?;
        let id_text = tangle.id.as_uuid().to_string();
        sqlx::query("UPDATE tangles SET detected_at = ?, resolved_at = ? WHERE id = ?")
            .bind(tangle.detected_at.unix_seconds())
            .bind(tangle.resolved_at.map(|t| t.unix_seconds()))
            .bind(&id_text)
            .execute(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to update tangle", e))?;
        sqlx::query("DELETE FROM tangle_tasks WHERE tangle_id = ?")
            .bind(&id_text)
            .execute(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to clear tangle tasks", e))?;
        for task_id in &tangle.task_ids {
            sqlx::query("INSERT INTO tangle_tasks (tangle_id, task_id) VALUES (?, ?)")
                .bind(&id_text)
                .bind(task_id.as_uuid().to_string())
                .execute(&mut *tx)
                .await
                .map_err(|e| RepoError::from_source("failed to insert tangle task", e))?;
        }
        tx.commit()
            .await
            .map_err(|e| RepoError::from_source("failed to commit tangle update", e))
    }
}

mod postgres_impl {
    use super::*;

    pub(super) async fn list_active(pool: &PgPool) -> Result<Vec<Tangle>, RepoError> {
        let rows = sqlx::query(
            "SELECT id, detected_at, resolved_at FROM tangles WHERE resolved_at IS NULL",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list active tangles", e))?;
        let mut tangles = Vec::with_capacity(rows.len());
        for row in rows {
            let id: uuid::Uuid = row.get("id");
            let task_rows = sqlx::query("SELECT task_id FROM tangle_tasks WHERE tangle_id = $1")
                .bind(id)
                .fetch_all(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to load tangle tasks", e))?;
            let task_ids = task_rows
                .into_iter()
                .map(|r| TaskId::new(r.get::<uuid::Uuid, _>("task_id")))
                .collect::<BTreeSet<_>>();
            tangles.push(assemble(
                id,
                task_ids,
                row.get::<i64, _>("detected_at"),
                row.get::<Option<i64>, _>("resolved_at"),
            )?);
        }
        Ok(tangles)
    }

    pub(super) async fn insert(pool: &PgPool, tangle: &Tangle) -> Result<(), RepoError> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| RepoError::from_source("failed to start transaction", e))?;
        sqlx::query("INSERT INTO tangles (id, detected_at, resolved_at) VALUES ($1, $2, $3)")
            .bind(tangle.id.as_uuid())
            .bind(tangle.detected_at.unix_seconds())
            .bind(tangle.resolved_at.map(|t| t.unix_seconds()))
            .execute(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to insert tangle", e))?;
        for task_id in &tangle.task_ids {
            sqlx::query("INSERT INTO tangle_tasks (tangle_id, task_id) VALUES ($1, $2)")
                .bind(tangle.id.as_uuid())
                .bind(task_id.as_uuid())
                .execute(&mut *tx)
                .await
                .map_err(|e| RepoError::from_source("failed to insert tangle task", e))?;
        }
        tx.commit()
            .await
            .map_err(|e| RepoError::from_source("failed to commit tangle insert", e))
    }

    pub(super) async fn update(pool: &PgPool, tangle: &Tangle) -> Result<(), RepoError> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| RepoError::from_source("failed to start transaction", e))?;
        sqlx::query("UPDATE tangles SET detected_at = $1, resolved_at = $2 WHERE id = $3")
            .bind(tangle.detected_at.unix_seconds())
            .bind(tangle.resolved_at.map(|t| t.unix_seconds()))
            .bind(tangle.id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to update tangle", e))?;
        sqlx::query("DELETE FROM tangle_tasks WHERE tangle_id = $1")
            .bind(tangle.id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to clear tangle tasks", e))?;
        for task_id in &tangle.task_ids {
            sqlx::query("INSERT INTO tangle_tasks (tangle_id, task_id) VALUES ($1, $2)")
                .bind(tangle.id.as_uuid())
                .bind(task_id.as_uuid())
                .execute(&mut *tx)
                .await
                .map_err(|e| RepoError::from_source("failed to insert tangle task", e))?;
        }
        tx.commit()
            .await
            .map_err(|e| RepoError::from_source("failed to commit tangle update", e))
    }
}

/// Reused by [`super::board_query`]'s `blocking_graph` (`BlockingGraph`
/// needs the same active-tangle set `TangleRepository::list_active` does).
pub(super) async fn list_active_sqlite(pool: &SqlitePool) -> Result<Vec<Tangle>, RepoError> {
    sqlite_impl::list_active(pool).await
}

/// Postgres sibling of [`list_active_sqlite`].
pub(super) async fn list_active_postgres(pool: &PgPool) -> Result<Vec<Tangle>, RepoError> {
    postgres_impl::list_active(pool).await
}

#[async_trait]
impl TangleRepository for SqlStore {
    async fn list_active(&self) -> Result<Vec<Tangle>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_active(pool).await,
            Backend::Postgres(pool) => postgres_impl::list_active(pool).await,
        }
    }

    async fn insert(&self, tangle: &Tangle) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::insert(pool, tangle).await,
            Backend::Postgres(pool) => postgres_impl::insert(pool, tangle).await,
        }
    }

    async fn update(&self, tangle: &Tangle) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::update(pool, tangle).await,
            Backend::Postgres(pool) => postgres_impl::update(pool, tangle).await,
        }
    }
}
