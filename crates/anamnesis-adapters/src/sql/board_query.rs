//! [`BoardQuery`] over [`SqlStore`] — `docs/DOMAIN.md` §7's "single most
//! important structural addition": the task board and the suggestion
//! engine's inputs are *queries* across everything above the horizon, never
//! one aggregate's load.
//!
//! Also carries [`SqlStore::seed_board_column`]: Phase D defined no port
//! that creates a [`Column`] (only [`BoardQuery`], which reads them) — see
//! the Phase E report for why this is a design-doc gap rather than an
//! oversight here. It is a real, if minimal, way to seed the three default
//! columns (`docs/DOMAIN.md` §3: To-Do/Doing/Done) and the one board
//! fixtures need in tests.

use std::collections::BTreeSet;

use anamnesis_app::{BoardColumn, BoardItem, BoardQuery, RepoError};
use anamnesis_core::{
    BlockingGraph, BoardState, Column, ColumnId, KindId, Tangle, Task, TaskSummary,
};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::tangle::{
    list_active_postgres, list_active_sqlite, list_by_column_postgres, list_by_column_sqlite,
};
use super::task::{TASK_COLUMNS, assemble_task};
use super::{Backend, SqlStore, parse_uuid, project_status_from_text, title_from_text};

/// Merges a column's tasks and placed tangles into one position-ordered
/// list — the "honest shape" `docs/DOMAIN.md` calls for: a heterogeneous
/// [`BoardItem`] list, not two parallel ones a caller would have to
/// re-interleave itself. Both inputs are individually well-ordered already
/// (`ORDER BY board_position`), and — since every placement in this phase
/// only ever *appends* at the column's current combined count
/// (`crate::use_cases::task::raise_task` / `tangle::place_tangle`, both via
/// `BoardQuery::count_on_column`) — positions across tasks and tangles in
/// the same column never collide, so a plain sort by position is enough.
fn interleave(tasks: Vec<Task>, tangles: Vec<Tangle>) -> Vec<BoardItem> {
    let mut items: Vec<BoardItem> = tasks
        .into_iter()
        .map(BoardItem::Task)
        .chain(tangles.into_iter().map(BoardItem::Tangle))
        .collect();
    items.sort_by_key(BoardItem::position);
    items
}

fn assemble_column(
    id: uuid::Uuid,
    title: String,
    position: i64,
    wip_limit: Option<i64>,
    is_done: bool,
) -> Result<Column, RepoError> {
    Ok(Column {
        id: ColumnId::new(id),
        title: title_from_text(title)?,
        position: u32::try_from(position)
            .map_err(|e| RepoError::from_source("invalid stored column position", e))?,
        wip_limit: wip_limit
            .map(u32::try_from)
            .transpose()
            .map_err(|e| RepoError::from_source("invalid stored wip_limit", e))?,
        is_done,
    })
}

mod sqlite_impl {
    use super::*;

    fn task_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Task, RepoError> {
        let column_id: Option<String> = row.get("column_id");
        let parent_id: Option<String> = row.get("parent_task_id");
        assemble_task(
            parse_uuid(&row.get::<String, _>("id"))?,
            parse_uuid(&row.get::<String, _>("project_id"))?,
            row.get("title"),
            row.get("description"),
            row.get("placement_kind"),
            column_id.map(|s| parse_uuid(&s)).transpose()?,
            row.get::<Option<i64>, _>("board_position"),
            parent_id.map(|s| parse_uuid(&s)).transpose()?,
            row.get::<i64, _>("checklist_position"),
            row.get::<i64, _>("created_at"),
            row.get::<i64, _>("last_touched_at"),
            row.get::<Option<i64>, _>("archived_at"),
            row.get::<i64, _>("bounce_count"),
            row.get::<Option<i64>, _>("last_bounced_at"),
            row.get::<Option<i64>, _>("last_offered_at"),
        )
    }

    pub(super) async fn columns_with_items(
        pool: &SqlitePool,
    ) -> Result<Vec<BoardColumn>, RepoError> {
        let column_rows = sqlx::query(
            "SELECT id, title, position, wip_limit, is_done FROM board_columns ORDER BY position",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list board columns", e))?;
        let mut result = Vec::with_capacity(column_rows.len());
        for row in column_rows {
            let column = assemble_column(
                parse_uuid(&row.get::<String, _>("id"))?,
                row.get("title"),
                row.get::<i64, _>("position"),
                row.get::<Option<i64>, _>("wip_limit"),
                row.get::<i64, _>("is_done") != 0,
            )?;
            let query = format!(
                "SELECT {TASK_COLUMNS} FROM tasks WHERE column_id = ? AND archived_at IS NULL \
                 ORDER BY board_position"
            );
            let task_rows = sqlx::query(sqlx::AssertSqlSafe(query))
                .bind(column.id.as_uuid().to_string())
                .fetch_all(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to list tasks for column", e))?;
            let tasks = task_rows
                .iter()
                .map(task_from_row)
                .collect::<Result<Vec<_>, _>>()?;
            let tangles_here = list_by_column_sqlite(pool, column.id.as_uuid()).await?;
            result.push(BoardColumn {
                column,
                items: interleave(tasks, tangles_here),
            });
        }
        Ok(result)
    }

    pub(super) async fn count_on_column(
        pool: &SqlitePool,
        column: ColumnId,
    ) -> Result<u32, RepoError> {
        let column_text = column.as_uuid().to_string();
        let tasks: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM tasks WHERE column_id = ? AND archived_at IS NULL",
        )
        .bind(&column_text)
        .fetch_one(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to count tasks on column", e))?;
        // A placed tangle counts against the column's WIP limit exactly
        // like a task (`docs/DOMAIN.md`'s Tangle section).
        let tangles: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM tangles WHERE column_id = ? AND resolved_at IS NULL",
        )
        .bind(&column_text)
        .fetch_one(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to count tangles on column", e))?;
        u32::try_from(tasks.0 + tangles.0)
            .map_err(|e| RepoError::from_source("board item count out of range", e))
    }

    pub(super) async fn board_state(
        pool: &SqlitePool,
        column: ColumnId,
    ) -> Result<BoardState, RepoError> {
        let wip_limit: Option<Option<i64>> =
            sqlx::query_scalar("SELECT wip_limit FROM board_columns WHERE id = ?")
                .bind(column.as_uuid().to_string())
                .fetch_optional(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to load column wip_limit", e))?;
        let wip_limit = wip_limit
            .flatten()
            .map(u32::try_from)
            .transpose()
            .map_err(|e| RepoError::from_source("invalid stored wip_limit", e))?;
        let current_count = count_on_column(pool, column).await?;
        Ok(BoardState {
            wip_limit,
            current_count,
        })
    }

    pub(super) async fn suggestion_candidates(
        pool: &SqlitePool,
    ) -> Result<Vec<TaskSummary>, RepoError> {
        let query = format!(
            "SELECT {}, p.status AS project_status FROM tasks t \
             JOIN projects p ON t.project_id = p.id",
            TASK_COLUMNS
                .split(", ")
                .map(|c| format!("t.{c} AS {c}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(query))
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to load suggestion candidates", e))?;
        rows.iter()
            .map(|row| {
                let task = task_from_row(row)?;
                let status = project_status_from_text(&row.get::<String, _>("project_status"))?;
                Ok(TaskSummary::from_task(&task, status))
            })
            .collect()
    }

    pub(super) async fn blocking_graph(pool: &SqlitePool) -> Result<BlockingGraph, RepoError> {
        let edge_rows =
            sqlx::query("SELECT from_task_id, to_task_id FROM relationships WHERE kind_id = ?")
                .bind(KindId::BUILTIN_BLOCKS.as_uuid().to_string())
                .fetch_all(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to load blocking edges", e))?;
        let edges = edge_rows
            .into_iter()
            .map(|row| {
                Ok((
                    anamnesis_core::TaskId::new(parse_uuid(&row.get::<String, _>("from_task_id"))?),
                    anamnesis_core::TaskId::new(parse_uuid(&row.get::<String, _>("to_task_id"))?),
                ))
            })
            .collect::<Result<Vec<_>, RepoError>>()?;

        let done_rows = sqlx::query(
            "SELECT t.id AS id FROM tasks t JOIN board_columns c ON t.column_id = c.id \
             WHERE c.is_done != 0 AND t.archived_at IS NULL",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load done tasks", e))?;
        let done_task_ids = done_rows
            .into_iter()
            .map(|row| {
                Ok(anamnesis_core::TaskId::new(parse_uuid(
                    &row.get::<String, _>("id"),
                )?))
            })
            .collect::<Result<BTreeSet<_>, RepoError>>()?;

        let tangles = list_active_sqlite(pool).await?;
        let tangled_task_ids: BTreeSet<_> = tangles
            .iter()
            .flat_map(|t: &Tangle| t.task_ids.iter().copied())
            .collect();

        Ok(BlockingGraph {
            edges,
            done_task_ids,
            tangled_task_ids,
            tangles,
        })
    }

    pub(super) async fn seed_board_column(
        pool: &SqlitePool,
        column: &Column,
    ) -> Result<(), RepoError> {
        let wip_limit = column.wip_limit.map(i64::from);
        sqlx::query(
            "INSERT INTO board_columns (id, title, position, wip_limit, is_done) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET title = excluded.title, position = excluded.position, \
             wip_limit = excluded.wip_limit, is_done = excluded.is_done",
        )
        .bind(column.id.as_uuid().to_string())
        .bind(column.title.as_str())
        .bind(i64::from(column.position))
        .bind(wip_limit)
        .bind(column.is_done)
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to seed board column", e))?;
        Ok(())
    }
}

mod postgres_impl {
    use super::*;

    fn task_from_row(row: &sqlx::postgres::PgRow) -> Result<Task, RepoError> {
        assemble_task(
            row.get::<uuid::Uuid, _>("id"),
            row.get::<uuid::Uuid, _>("project_id"),
            row.get("title"),
            row.get("description"),
            row.get("placement_kind"),
            row.get::<Option<uuid::Uuid>, _>("column_id"),
            row.get::<Option<i32>, _>("board_position").map(i64::from),
            row.get::<Option<uuid::Uuid>, _>("parent_task_id"),
            i64::from(row.get::<i32, _>("checklist_position")),
            row.get::<i64, _>("created_at"),
            row.get::<i64, _>("last_touched_at"),
            row.get::<Option<i64>, _>("archived_at"),
            i64::from(row.get::<i32, _>("bounce_count")),
            row.get::<Option<i64>, _>("last_bounced_at"),
            row.get::<Option<i64>, _>("last_offered_at"),
        )
    }

    pub(super) async fn columns_with_items(pool: &PgPool) -> Result<Vec<BoardColumn>, RepoError> {
        let column_rows = sqlx::query(
            "SELECT id, title, position, wip_limit, is_done FROM board_columns ORDER BY position",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to list board columns", e))?;
        let mut result = Vec::with_capacity(column_rows.len());
        for row in column_rows {
            let column = assemble_column(
                row.get::<uuid::Uuid, _>("id"),
                row.get("title"),
                i64::from(row.get::<i32, _>("position")),
                row.get::<Option<i32>, _>("wip_limit").map(i64::from),
                row.get("is_done"),
            )?;
            let query = format!(
                "SELECT {TASK_COLUMNS} FROM tasks WHERE column_id = $1 AND archived_at IS NULL \
                 ORDER BY board_position"
            );
            let task_rows = sqlx::query(sqlx::AssertSqlSafe(query))
                .bind(column.id.as_uuid())
                .fetch_all(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to list tasks for column", e))?;
            let tasks = task_rows
                .iter()
                .map(task_from_row)
                .collect::<Result<Vec<_>, _>>()?;
            let tangles_here = list_by_column_postgres(pool, column.id.as_uuid()).await?;
            result.push(BoardColumn {
                column,
                items: interleave(tasks, tangles_here),
            });
        }
        Ok(result)
    }

    pub(super) async fn count_on_column(pool: &PgPool, column: ColumnId) -> Result<u32, RepoError> {
        let tasks: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM tasks WHERE column_id = $1 AND archived_at IS NULL",
        )
        .bind(column.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to count tasks on column", e))?;
        let tangles: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM tangles WHERE column_id = $1 AND resolved_at IS NULL",
        )
        .bind(column.as_uuid())
        .fetch_one(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to count tangles on column", e))?;
        u32::try_from(tasks.0 + tangles.0)
            .map_err(|e| RepoError::from_source("board item count out of range", e))
    }

    pub(super) async fn board_state(
        pool: &PgPool,
        column: ColumnId,
    ) -> Result<BoardState, RepoError> {
        let wip_limit: Option<Option<i32>> =
            sqlx::query_scalar("SELECT wip_limit FROM board_columns WHERE id = $1")
                .bind(column.as_uuid())
                .fetch_optional(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to load column wip_limit", e))?;
        let wip_limit = wip_limit.flatten().map(|n| n as u32);
        let current_count = count_on_column(pool, column).await?;
        Ok(BoardState {
            wip_limit,
            current_count,
        })
    }

    pub(super) async fn suggestion_candidates(
        pool: &PgPool,
    ) -> Result<Vec<TaskSummary>, RepoError> {
        let query = format!(
            "SELECT {}, p.status AS project_status FROM tasks t \
             JOIN projects p ON t.project_id = p.id",
            TASK_COLUMNS
                .split(", ")
                .map(|c| format!("t.{c} AS {c}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(query))
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to load suggestion candidates", e))?;
        rows.iter()
            .map(|row| {
                let task = task_from_row(row)?;
                let status = project_status_from_text(&row.get::<String, _>("project_status"))?;
                Ok(TaskSummary::from_task(&task, status))
            })
            .collect()
    }

    pub(super) async fn blocking_graph(pool: &PgPool) -> Result<BlockingGraph, RepoError> {
        let edge_rows =
            sqlx::query("SELECT from_task_id, to_task_id FROM relationships WHERE kind_id = $1")
                .bind(KindId::BUILTIN_BLOCKS.as_uuid())
                .fetch_all(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to load blocking edges", e))?;
        let edges = edge_rows
            .into_iter()
            .map(|row| {
                (
                    anamnesis_core::TaskId::new(row.get::<uuid::Uuid, _>("from_task_id")),
                    anamnesis_core::TaskId::new(row.get::<uuid::Uuid, _>("to_task_id")),
                )
            })
            .collect::<Vec<_>>();

        let done_rows = sqlx::query(
            "SELECT t.id AS id FROM tasks t JOIN board_columns c ON t.column_id = c.id \
             WHERE c.is_done AND t.archived_at IS NULL",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load done tasks", e))?;
        let done_task_ids = done_rows
            .into_iter()
            .map(|row| anamnesis_core::TaskId::new(row.get::<uuid::Uuid, _>("id")))
            .collect::<BTreeSet<_>>();

        let tangles = list_active_postgres(pool).await?;
        let tangled_task_ids: BTreeSet<_> = tangles
            .iter()
            .flat_map(|t: &Tangle| t.task_ids.iter().copied())
            .collect();

        Ok(BlockingGraph {
            edges,
            done_task_ids,
            tangled_task_ids,
            tangles,
        })
    }

    pub(super) async fn seed_board_column(pool: &PgPool, column: &Column) -> Result<(), RepoError> {
        let position = i32::try_from(column.position)
            .map_err(|e| RepoError::from_source("column position out of range", e))?;
        let wip_limit = column
            .wip_limit
            .map(i32::try_from)
            .transpose()
            .map_err(|e| RepoError::from_source("wip_limit out of range", e))?;
        sqlx::query(
            "INSERT INTO board_columns (id, title, position, wip_limit, is_done) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (id) DO UPDATE SET title = excluded.title, position = excluded.position, \
             wip_limit = excluded.wip_limit, is_done = excluded.is_done",
        )
        .bind(column.id.as_uuid())
        .bind(column.title.as_str())
        .bind(position)
        .bind(wip_limit)
        .bind(column.is_done)
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to seed board column", e))?;
        Ok(())
    }
}

#[async_trait]
impl BoardQuery for SqlStore {
    async fn columns_with_items(&self) -> Result<Vec<BoardColumn>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::columns_with_items(pool).await,
            Backend::Postgres(pool) => postgres_impl::columns_with_items(pool).await,
        }
    }

    async fn count_on_column(&self, column: ColumnId) -> Result<u32, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::count_on_column(pool, column).await,
            Backend::Postgres(pool) => postgres_impl::count_on_column(pool, column).await,
        }
    }

    async fn board_state(&self, column: ColumnId) -> Result<BoardState, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::board_state(pool, column).await,
            Backend::Postgres(pool) => postgres_impl::board_state(pool, column).await,
        }
    }

    async fn suggestion_candidates(&self) -> Result<Vec<TaskSummary>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::suggestion_candidates(pool).await,
            Backend::Postgres(pool) => postgres_impl::suggestion_candidates(pool).await,
        }
    }

    async fn blocking_graph(&self) -> Result<BlockingGraph, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::blocking_graph(pool).await,
            Backend::Postgres(pool) => postgres_impl::blocking_graph(pool).await,
        }
    }
}

impl SqlStore {
    /// Creates or updates a global task-board [`Column`] (`docs/DOMAIN.md`
    /// §3). Not part of any Phase D port — see the module doc comment —
    /// but needed to seed the three default columns and to give tests a way
    /// to create fixture columns at all.
    pub async fn seed_board_column(&self, column: &Column) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::seed_board_column(pool, column).await,
            Backend::Postgres(pool) => postgres_impl::seed_board_column(pool, column).await,
        }
    }
}
