//! [`TangleRepository`] over [`SqlStore`]: system-derived, reconciled
//! against fresh detection passes (`docs/DOMAIN.md`'s Tangle section).
//!
//! `Tangle::fingerprint` is never stored: `anamnesis_core::Fingerprint`
//! exposes no accessor to its inner value, only the pure constructor
//! [`anamnesis_core::Fingerprint::of`] — so it is recomputed from the
//! loaded `task_ids` on every read instead, which is exactly what
//! `Fingerprint::of` is for and costs nothing extra to keep correct.
//!
//! `placement_kind`/`column_id`/`board_position` mirror `tasks`' own
//! placement encoding exactly (`super::task::encode_placement`/
//! `decode_placement`) — a placed tangle occupies a column slot precisely
//! like a task, so it round-trips through the identical scheme.

use std::collections::BTreeSet;

use anamnesis_app::{RepoError, TangleRepository};
use anamnesis_core::{Fingerprint, Tangle, TangleId, TaskId};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::task::{decode_placement, encode_placement};
use super::{Backend, SqlStore, parse_uuid, timestamp_from_seconds};

#[allow(clippy::too_many_arguments)]
fn assemble(
    id: uuid::Uuid,
    task_ids: BTreeSet<TaskId>,
    detected_at: i64,
    resolved_at: Option<i64>,
    placement_kind: &str,
    column_id: Option<uuid::Uuid>,
    board_position: Option<i64>,
    frozen: bool,
) -> Result<Tangle, RepoError> {
    Ok(Tangle {
        id: TangleId::new(id),
        fingerprint: Fingerprint::of(&task_ids),
        task_ids,
        placement: decode_placement(placement_kind, column_id, board_position)?,
        frozen,
        detected_at: timestamp_from_seconds(detected_at)?,
        resolved_at: resolved_at.map(timestamp_from_seconds).transpose()?,
    })
}

const TANGLE_COLUMNS: &str =
    "id, detected_at, resolved_at, placement_kind, column_id, board_position, frozen";

mod sqlite_impl {
    use super::*;

    async fn task_ids_for(pool: &SqlitePool, id_text: &str) -> Result<BTreeSet<TaskId>, RepoError> {
        let task_rows = sqlx::query("SELECT task_id FROM tangle_tasks WHERE tangle_id = ?")
            .bind(id_text)
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to load tangle tasks", e))?;
        task_rows
            .into_iter()
            .map(|r| Ok(TaskId::new(parse_uuid(&r.get::<String, _>("task_id"))?)))
            .collect()
    }

    fn tangle_from_row(
        row: &sqlx::sqlite::SqliteRow,
        task_ids: BTreeSet<TaskId>,
    ) -> Result<Tangle, RepoError> {
        let id = parse_uuid(&row.get::<String, _>("id"))?;
        let column_id: Option<String> = row.get("column_id");
        assemble(
            id,
            task_ids,
            row.get::<i64, _>("detected_at"),
            row.get::<Option<i64>, _>("resolved_at"),
            &row.get::<String, _>("placement_kind"),
            column_id.map(|s| parse_uuid(&s)).transpose()?,
            row.get::<Option<i64>, _>("board_position"),
            row.get::<i64, _>("frozen") != 0,
        )
    }

    pub(super) async fn list_active(pool: &SqlitePool) -> Result<Vec<Tangle>, RepoError> {
        let query = format!("SELECT {TANGLE_COLUMNS} FROM tangles WHERE resolved_at IS NULL");
        let rows = sqlx::query(sqlx::AssertSqlSafe(query))
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list active tangles", e))?;
        let mut tangles = Vec::with_capacity(rows.len());
        for row in rows {
            let id_text: String = row.get("id");
            let task_ids = task_ids_for(pool, &id_text).await?;
            tangles.push(tangle_from_row(&row, task_ids)?);
        }
        Ok(tangles)
    }

    /// Every tangle currently placed in `column`, **regardless of
    /// `resolved_at`** — unlike [`list_active`], on purpose: a just-resolved
    /// tangle must keep rendering in its (now `is_done`) column so the user
    /// sees the knot visibly close (`docs/DOMAIN.md`'s Tangle section)
    /// rather than vanish the instant it resolves, exactly as a `Task`
    /// stays visible in its column once done and before it is archived.
    pub(super) async fn list_by_column(
        pool: &SqlitePool,
        column: uuid::Uuid,
    ) -> Result<Vec<Tangle>, RepoError> {
        let query = format!("SELECT {TANGLE_COLUMNS} FROM tangles WHERE column_id = ?");
        let rows = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(column.to_string())
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list tangles for column", e))?;
        let mut tangles = Vec::with_capacity(rows.len());
        for row in rows {
            let id_text: String = row.get("id");
            let task_ids = task_ids_for(pool, &id_text).await?;
            tangles.push(tangle_from_row(&row, task_ids)?);
        }
        Ok(tangles)
    }

    pub(super) async fn load(pool: &SqlitePool, id: TangleId) -> Result<Option<Tangle>, RepoError> {
        let id_text = id.as_uuid().to_string();
        let query = format!("SELECT {TANGLE_COLUMNS} FROM tangles WHERE id = ?");
        let Some(row) = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(&id_text)
            .fetch_optional(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to load tangle", e))?
        else {
            return Ok(None);
        };
        let task_ids = task_ids_for(pool, &id_text).await?;
        Ok(Some(tangle_from_row(&row, task_ids)?))
    }

    pub(super) async fn insert(pool: &SqlitePool, tangle: &Tangle) -> Result<(), RepoError> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| RepoError::from_source("failed to start transaction", e))?;
        let (placement_kind, column_id, board_position) = encode_placement(&tangle.placement);
        sqlx::query(
            "INSERT INTO tangles \
             (id, detected_at, resolved_at, placement_kind, column_id, board_position, frozen) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(tangle.id.as_uuid().to_string())
        .bind(tangle.detected_at.unix_seconds())
        .bind(tangle.resolved_at.map(|t| t.unix_seconds()))
        .bind(placement_kind)
        .bind(column_id.map(|u| u.to_string()))
        .bind(board_position)
        .bind(tangle.frozen)
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
        let (placement_kind, column_id, board_position) = encode_placement(&tangle.placement);
        sqlx::query(
            "UPDATE tangles SET detected_at = ?, resolved_at = ?, placement_kind = ?, \
             column_id = ?, board_position = ?, frozen = ? WHERE id = ?",
        )
        .bind(tangle.detected_at.unix_seconds())
        .bind(tangle.resolved_at.map(|t| t.unix_seconds()))
        .bind(placement_kind)
        .bind(column_id.map(|u| u.to_string()))
        .bind(board_position)
        .bind(tangle.frozen)
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

    async fn task_ids_for(pool: &PgPool, id: uuid::Uuid) -> Result<BTreeSet<TaskId>, RepoError> {
        let task_rows = sqlx::query("SELECT task_id FROM tangle_tasks WHERE tangle_id = $1")
            .bind(id)
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to load tangle tasks", e))?;
        Ok(task_rows
            .into_iter()
            .map(|r| TaskId::new(r.get::<uuid::Uuid, _>("task_id")))
            .collect())
    }

    fn tangle_from_row(
        row: &sqlx::postgres::PgRow,
        task_ids: BTreeSet<TaskId>,
    ) -> Result<Tangle, RepoError> {
        assemble(
            row.get::<uuid::Uuid, _>("id"),
            task_ids,
            row.get::<i64, _>("detected_at"),
            row.get::<Option<i64>, _>("resolved_at"),
            &row.get::<String, _>("placement_kind"),
            row.get::<Option<uuid::Uuid>, _>("column_id"),
            row.get::<Option<i32>, _>("board_position").map(i64::from),
            row.get("frozen"),
        )
    }

    pub(super) async fn list_active(pool: &PgPool) -> Result<Vec<Tangle>, RepoError> {
        let query = format!("SELECT {TANGLE_COLUMNS} FROM tangles WHERE resolved_at IS NULL");
        let rows = sqlx::query(sqlx::AssertSqlSafe(query))
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list active tangles", e))?;
        let mut tangles = Vec::with_capacity(rows.len());
        for row in rows {
            let id: uuid::Uuid = row.get("id");
            let task_ids = task_ids_for(pool, id).await?;
            tangles.push(tangle_from_row(&row, task_ids)?);
        }
        Ok(tangles)
    }

    /// Postgres sibling of `sqlite_impl::list_by_column` — see its doc
    /// comment for why `resolved_at` is deliberately not filtered here.
    pub(super) async fn list_by_column(
        pool: &PgPool,
        column: uuid::Uuid,
    ) -> Result<Vec<Tangle>, RepoError> {
        let query = format!("SELECT {TANGLE_COLUMNS} FROM tangles WHERE column_id = $1");
        let rows = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(column)
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list tangles for column", e))?;
        let mut tangles = Vec::with_capacity(rows.len());
        for row in rows {
            let id: uuid::Uuid = row.get("id");
            let task_ids = task_ids_for(pool, id).await?;
            tangles.push(tangle_from_row(&row, task_ids)?);
        }
        Ok(tangles)
    }

    pub(super) async fn load(pool: &PgPool, id: TangleId) -> Result<Option<Tangle>, RepoError> {
        let query = format!("SELECT {TANGLE_COLUMNS} FROM tangles WHERE id = $1");
        let Some(row) = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(id.as_uuid())
            .fetch_optional(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to load tangle", e))?
        else {
            return Ok(None);
        };
        let task_ids = task_ids_for(pool, id.as_uuid()).await?;
        Ok(Some(tangle_from_row(&row, task_ids)?))
    }

    pub(super) async fn insert(pool: &PgPool, tangle: &Tangle) -> Result<(), RepoError> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| RepoError::from_source("failed to start transaction", e))?;
        let (placement_kind, column_id, board_position) = encode_placement(&tangle.placement);
        let board_position = board_position
            .map(i32::try_from)
            .transpose()
            .map_err(|e| RepoError::from_source("board position out of range", e))?;
        sqlx::query(
            "INSERT INTO tangles \
             (id, detected_at, resolved_at, placement_kind, column_id, board_position, frozen) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(tangle.id.as_uuid())
        .bind(tangle.detected_at.unix_seconds())
        .bind(tangle.resolved_at.map(|t| t.unix_seconds()))
        .bind(placement_kind)
        .bind(column_id)
        .bind(board_position)
        .bind(tangle.frozen)
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
        let (placement_kind, column_id, board_position) = encode_placement(&tangle.placement);
        let board_position = board_position
            .map(i32::try_from)
            .transpose()
            .map_err(|e| RepoError::from_source("board position out of range", e))?;
        sqlx::query(
            "UPDATE tangles SET detected_at = $1, resolved_at = $2, placement_kind = $3, \
             column_id = $4, board_position = $5, frozen = $6 WHERE id = $7",
        )
        .bind(tangle.detected_at.unix_seconds())
        .bind(tangle.resolved_at.map(|t| t.unix_seconds()))
        .bind(placement_kind)
        .bind(column_id)
        .bind(board_position)
        .bind(tangle.frozen)
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

/// Reused by [`super::board_query`]'s `blocking_graph` and column-item
/// listing (`BlockingGraph` and `BoardColumn` both need the active-tangle
/// set `TangleRepository::list_active` does).
pub(super) async fn list_active_sqlite(pool: &SqlitePool) -> Result<Vec<Tangle>, RepoError> {
    sqlite_impl::list_active(pool).await
}

/// Postgres sibling of [`list_active_sqlite`].
pub(super) async fn list_active_postgres(pool: &PgPool) -> Result<Vec<Tangle>, RepoError> {
    postgres_impl::list_active(pool).await
}

/// Reused by [`super::board_query`] for one column's item list — every
/// tangle placed there, resolved or not (see `sqlite_impl::list_by_column`'s
/// doc comment).
pub(super) async fn list_by_column_sqlite(
    pool: &SqlitePool,
    column: uuid::Uuid,
) -> Result<Vec<Tangle>, RepoError> {
    sqlite_impl::list_by_column(pool, column).await
}

/// Postgres sibling of [`list_by_column_sqlite`].
pub(super) async fn list_by_column_postgres(
    pool: &PgPool,
    column: uuid::Uuid,
) -> Result<Vec<Tangle>, RepoError> {
    postgres_impl::list_by_column(pool, column).await
}

#[async_trait]
impl TangleRepository for SqlStore {
    async fn list_active(&self) -> Result<Vec<Tangle>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_active(pool).await,
            Backend::Postgres(pool) => postgres_impl::list_active(pool).await,
        }
    }

    async fn load(&self, id: TangleId) -> Result<Option<Tangle>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::load(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::load(pool, id).await,
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
