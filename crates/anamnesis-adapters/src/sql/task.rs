//! [`TaskRepository`] over [`SqlStore`]: a [`Task`] loaded with its
//! [`FieldValue`]s (`docs/DOMAIN.md` §7). `update` is the one
//! optimistic-concurrency-checked write in the crate.
//!
//! [`FieldValue`]'s typed EAV storage (`docs/DOMAIN.md` §3: "separate
//! value_int / value_text / value_ts columns... not JSON") encodes each
//! [`FieldData`] variant as:
//! - `Number` -> `value_int` (units), `value_num_scale` (scale)
//! - `Currency` -> `value_int` (minor units), `value_currency_code`
//! - `Date` -> `value_ts`, as a Julian day number (`time::Date::to_julian_day`)
//! - `Time` -> `value_ts`, as whole seconds since local midnight (sub-second
//!   precision, if any, is not representable and is not needed here)
//! - `DateTime` -> `value_ts`, as Unix seconds
//! - `Line`/`Block` -> `value_text`
//!
//! Decoding consults the owning `field_definitions.kind` (joined in) to know
//! which columns to read — the same discriminant [`FieldData::kind`] already
//! carries in memory.

use anamnesis_app::{RepoError, TaskAggregate, TaskRepository, TaskUpdateError};
use anamnesis_core::{
    ColumnId, CurrencyAmount, CurrencyCode, FieldData, FieldId, FieldKind, NumberValue, Placement,
    ProjectId, Task, TaskId, Timestamp,
};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::{
    Backend, SqlStore, field_kind_from_text, parse_uuid, timestamp_from_seconds, title_from_text,
};

// --- Date/Time <-> integer encodings shared by both backends. ---

fn epoch_julian_day() -> i32 {
    time::Date::from_calendar_date(1970, time::Month::January, 1)
        .expect("1970-01-01 is a valid date")
        .to_julian_day()
}

fn date_to_days(date: time::Date) -> i64 {
    i64::from(date.to_julian_day() - epoch_julian_day())
}

fn days_to_date(days: i64) -> Result<time::Date, RepoError> {
    let julian = i32::try_from(days)
        .ok()
        .and_then(|d| d.checked_add(epoch_julian_day()))
        .ok_or_else(|| RepoError::new("stored date out of range"))?;
    time::Date::from_julian_day(julian)
        .map_err(|e| RepoError::from_source("invalid stored date", e))
}

fn time_to_seconds(t: time::Time) -> i64 {
    i64::from(t.hour()) * 3600 + i64::from(t.minute()) * 60 + i64::from(t.second())
}

fn seconds_to_time(secs: i64) -> Result<time::Time, RepoError> {
    if !(0..86_400).contains(&secs) {
        return Err(RepoError::new("stored time-of-day out of range"));
    }
    let h = (secs / 3600) as u8;
    let m = ((secs % 3600) / 60) as u8;
    let s = (secs % 60) as u8;
    time::Time::from_hms(h, m, s).map_err(|e| RepoError::from_source("invalid stored time", e))
}

/// The typed-EAV columns for one [`FieldData`], independent of backend.
#[derive(Debug, Default)]
struct EncodedFieldData {
    value_int: Option<i64>,
    value_num_scale: Option<i32>,
    value_currency_code: Option<String>,
    value_text: Option<String>,
    value_ts: Option<i64>,
}

fn encode_field_data(data: &FieldData) -> EncodedFieldData {
    match data {
        FieldData::Number(n) => EncodedFieldData {
            value_int: Some(n.units),
            value_num_scale: Some(i32::from(n.scale)),
            ..Default::default()
        },
        FieldData::Currency(c) => EncodedFieldData {
            value_int: Some(c.minor_units),
            value_currency_code: Some(c.currency.as_str().to_string()),
            ..Default::default()
        },
        FieldData::Date(d) => EncodedFieldData {
            value_ts: Some(date_to_days(*d)),
            ..Default::default()
        },
        FieldData::Time(t) => EncodedFieldData {
            value_ts: Some(time_to_seconds(*t)),
            ..Default::default()
        },
        FieldData::DateTime(ts) => EncodedFieldData {
            value_ts: Some(ts.unix_seconds()),
            ..Default::default()
        },
        FieldData::Line(s) => EncodedFieldData {
            value_text: Some(s.clone()),
            ..Default::default()
        },
        FieldData::Block(s) => EncodedFieldData {
            value_text: Some(s.clone()),
            ..Default::default()
        },
    }
}

/// A stored field value is missing the column its `FieldKind` requires —
/// shared by every arm of [`decode_field_data`] and its per-kind helpers,
/// replacing what was previously a closure re-created on every call.
fn missing(col: &str) -> RepoError {
    RepoError::new(format!("stored field value missing {col}"))
}

fn decode_number(
    value_int: Option<i64>,
    value_num_scale: Option<i32>,
) -> Result<FieldData, RepoError> {
    let units = value_int.ok_or_else(|| missing("value_int"))?;
    let scale = value_num_scale.ok_or_else(|| missing("value_num_scale"))?;
    let scale = u8::try_from(scale)
        .map_err(|e| RepoError::from_source("stored number scale out of range", e))?;
    Ok(FieldData::Number(NumberValue { units, scale }))
}

fn decode_currency(
    value_int: Option<i64>,
    value_currency_code: Option<String>,
) -> Result<FieldData, RepoError> {
    let minor_units = value_int.ok_or_else(|| missing("value_int"))?;
    let code = value_currency_code.ok_or_else(|| missing("value_currency_code"))?;
    let currency = CurrencyCode::new(&code)
        .map_err(|e| RepoError::from_source("invalid stored currency code", e))?;
    Ok(FieldData::Currency(CurrencyAmount {
        minor_units,
        currency,
    }))
}

/// The three `FieldKind` variants that share one stored column
/// (`value_ts`) and differ only in how it is converted back to a value.
fn decode_temporal(kind: FieldKind, value_ts: Option<i64>) -> Result<FieldData, RepoError> {
    let ts = value_ts.ok_or_else(|| missing("value_ts"))?;
    match kind {
        FieldKind::Date => Ok(FieldData::Date(days_to_date(ts)?)),
        FieldKind::Time => Ok(FieldData::Time(seconds_to_time(ts)?)),
        FieldKind::DateTime => Ok(FieldData::DateTime(timestamp_from_seconds(ts)?)),
        _ => unreachable!("decode_temporal is only called for Date/Time/DateTime"),
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_field_data(
    kind: FieldKind,
    value_int: Option<i64>,
    value_num_scale: Option<i32>,
    value_currency_code: Option<String>,
    value_text: Option<String>,
    value_ts: Option<i64>,
) -> Result<FieldData, RepoError> {
    match kind {
        FieldKind::Number => decode_number(value_int, value_num_scale),
        FieldKind::Currency => decode_currency(value_int, value_currency_code),
        FieldKind::Date | FieldKind::Time | FieldKind::DateTime => decode_temporal(kind, value_ts),
        FieldKind::Line => Ok(FieldData::Line(
            value_text.ok_or_else(|| missing("value_text"))?,
        )),
        FieldKind::Block => Ok(FieldData::Block(
            value_text.ok_or_else(|| missing("value_text"))?,
        )),
    }
}

// --- Placement <-> (placement_kind, column_id, board_position). ---

pub(super) fn encode_placement(
    placement: &Placement,
) -> (&'static str, Option<uuid::Uuid>, Option<i64>) {
    match placement {
        Placement::Below => ("below", None, None),
        Placement::OnBoard { column, position } => (
            "on_board",
            Some(column.as_uuid()),
            Some(i64::from(*position)),
        ),
    }
}

pub(super) fn decode_placement(
    kind: &str,
    column_id: Option<uuid::Uuid>,
    position: Option<i64>,
) -> Result<Placement, RepoError> {
    match kind {
        "below" => Ok(Placement::Below),
        "on_board" => {
            let column =
                column_id.ok_or_else(|| RepoError::new("on_board task missing column_id"))?;
            let position =
                position.ok_or_else(|| RepoError::new("on_board task missing board_position"))?;
            let position = u32::try_from(position)
                .map_err(|e| RepoError::from_source("stored board position out of range", e))?;
            Ok(Placement::OnBoard {
                column: ColumnId::new(column),
                position,
            })
        }
        other => Err(RepoError::new(format!(
            "invalid stored placement kind {other:?}"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn assemble_task(
    id: uuid::Uuid,
    project_id: uuid::Uuid,
    title: String,
    description: String,
    placement_kind: String,
    column_id: Option<uuid::Uuid>,
    board_position: Option<i64>,
    parent_task_id: Option<uuid::Uuid>,
    checklist_position: i64,
    created_at: i64,
    last_touched_at: i64,
    archived_at: Option<i64>,
    bounce_count: i64,
    last_bounced_at: Option<i64>,
    last_offered_at: Option<i64>,
) -> Result<Task, RepoError> {
    Ok(Task {
        id: TaskId::new(id),
        project_id: ProjectId::new(project_id),
        title: title_from_text(title)?,
        description,
        placement: decode_placement(&placement_kind, column_id, board_position)?,
        parent_task_id: parent_task_id.map(TaskId::new),
        checklist_position: u32::try_from(checklist_position)
            .map_err(|e| RepoError::from_source("stored checklist position out of range", e))?,
        created_at: timestamp_from_seconds(created_at)?,
        last_touched_at: timestamp_from_seconds(last_touched_at)?,
        archived_at: archived_at.map(timestamp_from_seconds).transpose()?,
        bounce_count: u32::try_from(bounce_count)
            .map_err(|e| RepoError::from_source("stored bounce count out of range", e))?,
        last_bounced_at: last_bounced_at.map(timestamp_from_seconds).transpose()?,
        last_offered_at: last_offered_at.map(timestamp_from_seconds).transpose()?,
    })
}

pub(super) const TASK_COLUMNS: &str = "id, project_id, title, description, placement_kind, column_id, \
     board_position, parent_task_id, checklist_position, created_at, last_touched_at, \
     archived_at, bounce_count, last_bounced_at, last_offered_at";

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

    pub(super) async fn load(
        pool: &SqlitePool,
        id: TaskId,
    ) -> Result<Option<TaskAggregate>, RepoError> {
        let id_text = id.as_uuid().to_string();
        let query = format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?");
        let Some(row) = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(&id_text)
            .fetch_optional(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to load task", e))?
        else {
            return Ok(None);
        };
        let task = task_from_row(&row)?;

        let value_rows = sqlx::query(
            "SELECT fv.field_id AS field_id, fd.kind AS kind, fv.value_int AS value_int, \
             fv.value_num_scale AS value_num_scale, fv.value_currency_code AS value_currency_code, \
             fv.value_text AS value_text, fv.value_ts AS value_ts \
             FROM field_values fv JOIN field_definitions fd ON fv.field_id = fd.id \
             WHERE fv.task_id = ?",
        )
        .bind(&id_text)
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load field values", e))?;
        let field_values = value_rows
            .into_iter()
            .map(|row| {
                let field_id = parse_uuid(&row.get::<String, _>("field_id"))?;
                let kind = field_kind_from_text(&row.get::<String, _>("kind"))?;
                let data = decode_field_data(
                    kind,
                    row.get("value_int"),
                    row.get::<Option<i64>, _>("value_num_scale")
                        .map(|v| v as i32),
                    row.get("value_currency_code"),
                    row.get("value_text"),
                    row.get("value_ts"),
                )?;
                Ok(anamnesis_core::FieldValue {
                    field_id: FieldId::new(field_id),
                    task_id: id,
                    data,
                })
            })
            .collect::<Result<Vec<_>, RepoError>>()?;

        Ok(Some(TaskAggregate { task, field_values }))
    }

    pub(super) async fn list_children(
        pool: &SqlitePool,
        parent_id: TaskId,
    ) -> Result<Vec<Task>, RepoError> {
        let query = format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE parent_task_id = ? AND archived_at IS NULL \
             ORDER BY checklist_position"
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(parent_id.as_uuid().to_string())
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list task children", e))?;
        rows.iter().map(task_from_row).collect()
    }

    pub(super) async fn list_by_project(
        pool: &SqlitePool,
        project_id: ProjectId,
    ) -> Result<Vec<Task>, RepoError> {
        let query = format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE project_id = ? AND archived_at IS NULL \
             ORDER BY created_at"
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(project_id.as_uuid().to_string())
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list tasks for project", e))?;
        rows.iter().map(task_from_row).collect()
    }

    pub(super) async fn insert(pool: &SqlitePool, task: &Task) -> Result<(), RepoError> {
        let (placement_kind, column_id, board_position) = encode_placement(&task.placement);
        sqlx::query(
            "INSERT INTO tasks \
             (id, project_id, title, description, placement_kind, column_id, board_position, \
              parent_task_id, checklist_position, created_at, last_touched_at, archived_at, \
              bounce_count, last_bounced_at, last_offered_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(task.id.as_uuid().to_string())
        .bind(task.project_id.as_uuid().to_string())
        .bind(task.title.as_str())
        .bind(&task.description)
        .bind(placement_kind)
        .bind(column_id.map(|u| u.to_string()))
        .bind(board_position)
        .bind(task.parent_task_id.map(|p| p.as_uuid().to_string()))
        .bind(i64::from(task.checklist_position))
        .bind(task.created_at.unix_seconds())
        .bind(task.last_touched_at.unix_seconds())
        .bind(task.archived_at.map(|t| t.unix_seconds()))
        .bind(i64::from(task.bounce_count))
        .bind(task.last_bounced_at.map(|t| t.unix_seconds()))
        .bind(task.last_offered_at.map(|t| t.unix_seconds()))
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert task", e))?;
        Ok(())
    }

    /// Binds every column `update` writes onto the `UPDATE tasks ...`
    /// statement. Split out of `update` so that function reads as
    /// "check the optimistic-concurrency precondition, then write the
    /// row" instead of interleaving the two dozen column binds with the
    /// transaction/conflict-check control flow around them.
    fn bind_task_update<'a>(
        task: &'a Task,
        id_text: &'a str,
    ) -> sqlx::query::Query<'a, sqlx::Sqlite, sqlx::sqlite::SqliteArguments> {
        let (placement_kind, column_id, board_position) = encode_placement(&task.placement);
        sqlx::query(
            "UPDATE tasks SET title = ?, description = ?, placement_kind = ?, column_id = ?, \
             board_position = ?, parent_task_id = ?, checklist_position = ?, \
             last_touched_at = ?, archived_at = ?, bounce_count = ?, last_bounced_at = ?, \
             last_offered_at = ? WHERE id = ?",
        )
        .bind(task.title.as_str())
        .bind(&task.description)
        .bind(placement_kind)
        .bind(column_id.map(|u| u.to_string()))
        .bind(board_position)
        .bind(task.parent_task_id.map(|p| p.as_uuid().to_string()))
        .bind(i64::from(task.checklist_position))
        .bind(task.last_touched_at.unix_seconds())
        .bind(task.archived_at.map(|t| t.unix_seconds()))
        .bind(i64::from(task.bounce_count))
        .bind(task.last_bounced_at.map(|t| t.unix_seconds()))
        .bind(task.last_offered_at.map(|t| t.unix_seconds()))
        .bind(id_text)
    }

    pub(super) async fn update(
        pool: &SqlitePool,
        task: &Task,
        expected_last_touched_at: Timestamp,
    ) -> Result<(), TaskUpdateError> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| RepoError::from_source("failed to start transaction", e))?;
        let id_text = task.id.as_uuid().to_string();

        let current: Option<i64> =
            sqlx::query_scalar("SELECT last_touched_at FROM tasks WHERE id = ?")
                .bind(&id_text)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|e| RepoError::from_source("failed to read task for update", e))?;
        match current {
            None => return Err(TaskUpdateError::Repo(RepoError::new("no such task"))),
            Some(v) if v != expected_last_touched_at.unix_seconds() => {
                return Err(TaskUpdateError::Conflict);
            }
            _ => {}
        }

        bind_task_update(task, &id_text)
            .execute(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to update task", e))?;

        tx.commit()
            .await
            .map_err(|e| RepoError::from_source("failed to commit task update", e))?;
        Ok(())
    }

    pub(super) async fn set_field_value(
        pool: &SqlitePool,
        value: &anamnesis_core::FieldValue,
    ) -> Result<(), RepoError> {
        let encoded = encode_field_data(&value.data);
        sqlx::query(
            "INSERT INTO field_values \
             (field_id, task_id, value_int, value_num_scale, value_currency_code, value_text, value_ts) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(field_id, task_id) DO UPDATE SET \
             value_int = excluded.value_int, value_num_scale = excluded.value_num_scale, \
             value_currency_code = excluded.value_currency_code, value_text = excluded.value_text, \
             value_ts = excluded.value_ts",
        )
        .bind(value.field_id.as_uuid().to_string())
        .bind(value.task_id.as_uuid().to_string())
        .bind(encoded.value_int)
        .bind(encoded.value_num_scale)
        .bind(encoded.value_currency_code)
        .bind(encoded.value_text)
        .bind(encoded.value_ts)
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to set field value", e))?;
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

    pub(super) async fn load(
        pool: &PgPool,
        id: TaskId,
    ) -> Result<Option<TaskAggregate>, RepoError> {
        let query = format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = $1");
        let Some(row) = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(id.as_uuid())
            .fetch_optional(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to load task", e))?
        else {
            return Ok(None);
        };
        let task = task_from_row(&row)?;

        let value_rows = sqlx::query(
            "SELECT fv.field_id AS field_id, fd.kind AS kind, fv.value_int AS value_int, \
             fv.value_num_scale AS value_num_scale, fv.value_currency_code AS value_currency_code, \
             fv.value_text AS value_text, fv.value_ts AS value_ts \
             FROM field_values fv JOIN field_definitions fd ON fv.field_id = fd.id \
             WHERE fv.task_id = $1",
        )
        .bind(id.as_uuid())
        .fetch_all(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load field values", e))?;
        let field_values = value_rows
            .into_iter()
            .map(|row| {
                let field_id = row.get::<uuid::Uuid, _>("field_id");
                let kind = field_kind_from_text(&row.get::<String, _>("kind"))?;
                let data = decode_field_data(
                    kind,
                    row.get("value_int"),
                    row.get("value_num_scale"),
                    row.get("value_currency_code"),
                    row.get("value_text"),
                    row.get("value_ts"),
                )?;
                Ok(anamnesis_core::FieldValue {
                    field_id: FieldId::new(field_id),
                    task_id: id,
                    data,
                })
            })
            .collect::<Result<Vec<_>, RepoError>>()?;

        Ok(Some(TaskAggregate { task, field_values }))
    }

    pub(super) async fn list_children(
        pool: &PgPool,
        parent_id: TaskId,
    ) -> Result<Vec<Task>, RepoError> {
        let query = format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE parent_task_id = $1 AND archived_at IS NULL \
             ORDER BY checklist_position"
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(parent_id.as_uuid())
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list task children", e))?;
        rows.iter().map(task_from_row).collect()
    }

    pub(super) async fn list_by_project(
        pool: &PgPool,
        project_id: ProjectId,
    ) -> Result<Vec<Task>, RepoError> {
        let query = format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE project_id = $1 AND archived_at IS NULL \
             ORDER BY created_at"
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(project_id.as_uuid())
            .fetch_all(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to list tasks for project", e))?;
        rows.iter().map(task_from_row).collect()
    }

    pub(super) async fn insert(pool: &PgPool, task: &Task) -> Result<(), RepoError> {
        let (placement_kind, column_id, board_position) = encode_placement(&task.placement);
        let board_position = board_position
            .map(i32::try_from)
            .transpose()
            .map_err(|e| RepoError::from_source("board position out of range", e))?;
        let checklist_position = i32::try_from(task.checklist_position)
            .map_err(|e| RepoError::from_source("checklist position out of range", e))?;
        let bounce_count = i32::try_from(task.bounce_count)
            .map_err(|e| RepoError::from_source("bounce count out of range", e))?;
        sqlx::query(
            "INSERT INTO tasks \
             (id, project_id, title, description, placement_kind, column_id, board_position, \
              parent_task_id, checklist_position, created_at, last_touched_at, archived_at, \
              bounce_count, last_bounced_at, last_offered_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
        )
        .bind(task.id.as_uuid())
        .bind(task.project_id.as_uuid())
        .bind(task.title.as_str())
        .bind(&task.description)
        .bind(placement_kind)
        .bind(column_id)
        .bind(board_position)
        .bind(task.parent_task_id.map(|p| p.as_uuid()))
        .bind(checklist_position)
        .bind(task.created_at.unix_seconds())
        .bind(task.last_touched_at.unix_seconds())
        .bind(task.archived_at.map(|t| t.unix_seconds()))
        .bind(bounce_count)
        .bind(task.last_bounced_at.map(|t| t.unix_seconds()))
        .bind(task.last_offered_at.map(|t| t.unix_seconds()))
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to insert task", e))?;
        Ok(())
    }

    /// See `sqlite_impl::bind_task_update` — same split. Fallible here
    /// because Postgres's `int4` columns need the usual range-checked
    /// narrowing from the domain's wider integer types.
    fn bind_task_update(
        task: &Task,
    ) -> Result<sqlx::query::Query<'_, sqlx::Postgres, sqlx::postgres::PgArguments>, RepoError>
    {
        let (placement_kind, column_id, board_position) = encode_placement(&task.placement);
        let board_position = board_position
            .map(i32::try_from)
            .transpose()
            .map_err(|e| RepoError::from_source("board position out of range", e))?;
        let checklist_position = i32::try_from(task.checklist_position)
            .map_err(|e| RepoError::from_source("checklist position out of range", e))?;
        let bounce_count = i32::try_from(task.bounce_count)
            .map_err(|e| RepoError::from_source("bounce count out of range", e))?;
        Ok(sqlx::query(
            "UPDATE tasks SET title = $1, description = $2, placement_kind = $3, column_id = $4, \
             board_position = $5, parent_task_id = $6, checklist_position = $7, \
             last_touched_at = $8, archived_at = $9, bounce_count = $10, last_bounced_at = $11, \
             last_offered_at = $12 WHERE id = $13",
        )
        .bind(task.title.as_str())
        .bind(&task.description)
        .bind(placement_kind)
        .bind(column_id)
        .bind(board_position)
        .bind(task.parent_task_id.map(|p| p.as_uuid()))
        .bind(checklist_position)
        .bind(task.last_touched_at.unix_seconds())
        .bind(task.archived_at.map(|t| t.unix_seconds()))
        .bind(bounce_count)
        .bind(task.last_bounced_at.map(|t| t.unix_seconds()))
        .bind(task.last_offered_at.map(|t| t.unix_seconds()))
        .bind(task.id.as_uuid()))
    }

    pub(super) async fn update(
        pool: &PgPool,
        task: &Task,
        expected_last_touched_at: Timestamp,
    ) -> Result<(), TaskUpdateError> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| RepoError::from_source("failed to start transaction", e))?;

        let current: Option<i64> =
            sqlx::query_scalar("SELECT last_touched_at FROM tasks WHERE id = $1 FOR UPDATE")
                .bind(task.id.as_uuid())
                .fetch_optional(&mut *tx)
                .await
                .map_err(|e| RepoError::from_source("failed to read task for update", e))?;
        match current {
            None => return Err(TaskUpdateError::Repo(RepoError::new("no such task"))),
            Some(v) if v != expected_last_touched_at.unix_seconds() => {
                return Err(TaskUpdateError::Conflict);
            }
            _ => {}
        }

        bind_task_update(task)?
            .execute(&mut *tx)
            .await
            .map_err(|e| RepoError::from_source("failed to update task", e))?;

        tx.commit()
            .await
            .map_err(|e| RepoError::from_source("failed to commit task update", e))?;
        Ok(())
    }

    pub(super) async fn set_field_value(
        pool: &PgPool,
        value: &anamnesis_core::FieldValue,
    ) -> Result<(), RepoError> {
        let encoded = encode_field_data(&value.data);
        sqlx::query(
            "INSERT INTO field_values \
             (field_id, task_id, value_int, value_num_scale, value_currency_code, value_text, value_ts) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (field_id, task_id) DO UPDATE SET \
             value_int = excluded.value_int, value_num_scale = excluded.value_num_scale, \
             value_currency_code = excluded.value_currency_code, value_text = excluded.value_text, \
             value_ts = excluded.value_ts",
        )
        .bind(value.field_id.as_uuid())
        .bind(value.task_id.as_uuid())
        .bind(encoded.value_int)
        .bind(encoded.value_num_scale)
        .bind(encoded.value_currency_code)
        .bind(encoded.value_text)
        .bind(encoded.value_ts)
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to set field value", e))?;
        Ok(())
    }
}

#[async_trait]
impl TaskRepository for SqlStore {
    async fn load(&self, id: TaskId) -> Result<Option<TaskAggregate>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::load(pool, id).await,
            Backend::Postgres(pool) => postgres_impl::load(pool, id).await,
        }
    }

    async fn list_children(&self, parent_id: TaskId) -> Result<Vec<Task>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_children(pool, parent_id).await,
            Backend::Postgres(pool) => postgres_impl::list_children(pool, parent_id).await,
        }
    }

    async fn list_by_project(&self, project_id: ProjectId) -> Result<Vec<Task>, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::list_by_project(pool, project_id).await,
            Backend::Postgres(pool) => postgres_impl::list_by_project(pool, project_id).await,
        }
    }

    async fn insert(&self, task: &Task) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::insert(pool, task).await,
            Backend::Postgres(pool) => postgres_impl::insert(pool, task).await,
        }
    }

    async fn update(
        &self,
        task: &Task,
        expected_last_touched_at: Timestamp,
    ) -> Result<(), TaskUpdateError> {
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlite_impl::update(pool, task, expected_last_touched_at).await
            }
            Backend::Postgres(pool) => {
                postgres_impl::update(pool, task, expected_last_touched_at).await
            }
        }
    }

    async fn set_field_value(&self, value: &anamnesis_core::FieldValue) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::set_field_value(pool, value).await,
            Backend::Postgres(pool) => postgres_impl::set_field_value(pool, value).await,
        }
    }
}
