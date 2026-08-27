//! [`Column`]: a global task-board column (`docs/DOMAIN.md` §3).
//!
//! Columns are global — the task board spans all active projects and areas,
//! and its WIP limits apply across all of them. Contrast with the *project*
//! board, whose columns are fixed (`Pending` / `Active` / `Complete`,
//! derived from [`crate::ProjectStatus`]) and are not represented by this
//! type at all.
//!
//! Enforcing a WIP limit requires counting how many tasks currently sit in
//! a column — a query over the task board, not something this single
//! `Column` value can check in isolation. That check belongs to the Phase D
//! use case that places a task on the board; this module only carries the
//! `Column` entity and its own field edits.

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::ColumnId;
use crate::title::Title;

/// A global task-board column. Defaults, per the design: **To-Do**
/// (WIP-limited), **Doing**, **Done**.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub id: ColumnId,
    pub title: Title,
    pub position: u32,
    pub wip_limit: Option<u32>,
    /// Whether landing in this column counts as "done" — gates archive
    /// sweeps (Phase C) and the suggestion engine's "unblocked" rule
    /// (Phase B).
    pub is_done: bool,
}

/// Creates a new column.
pub fn create_column(
    id: ColumnId,
    title: impl AsRef<str>,
    position: u32,
    wip_limit: Option<u32>,
    is_done: bool,
) -> Result<Column, DomainError> {
    Ok(Column {
        id,
        title: Title::new(title)?,
        position,
        wip_limit,
        is_done,
    })
}

/// Renames a column.
pub fn rename_column(column: &Column, title: impl AsRef<str>) -> Result<Column, DomainError> {
    Ok(Column {
        title: Title::new(title)?,
        ..column.clone()
    })
}

/// Moves a column to a new position among its siblings.
pub fn reposition_column(column: &Column, position: u32) -> Column {
    Column {
        position,
        ..column.clone()
    }
}

/// Sets (or clears) a column's WIP limit.
pub fn set_wip_limit(column: &Column, wip_limit: Option<u32>) -> Column {
    Column {
        wip_limit,
        ..column.clone()
    }
}

/// Sets whether a column counts as "done".
pub fn set_is_done(column: &Column, is_done: bool) -> Column {
    Column {
        is_done,
        ..column.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn cid(n: u128) -> ColumnId {
        ColumnId::new(Uuid::from_u128(n))
    }

    fn column() -> Column {
        create_column(cid(1), "To-Do", 0, Some(5), false).unwrap()
    }

    #[test]
    fn create_column_builds_a_column_with_the_given_fields() {
        let c = column();
        assert_eq!(c.title.as_str(), "To-Do");
        assert_eq!(c.position, 0);
        assert_eq!(c.wip_limit, Some(5));
        assert!(!c.is_done);
    }

    #[test]
    fn create_column_rejects_an_invalid_title() {
        let result = create_column(cid(1), "", 0, None, false);
        assert!(matches!(result, Err(DomainError::InvalidTitle(_))));
    }

    #[test]
    fn rename_column_replaces_the_title() {
        let c = column();
        let renamed = rename_column(&c, "Doing").unwrap();
        assert_eq!(renamed.title.as_str(), "Doing");
    }

    #[test]
    fn reposition_set_wip_limit_and_set_is_done_update_independently() {
        let c = column();
        assert_eq!(reposition_column(&c, 2).position, 2);
        assert_eq!(set_wip_limit(&c, None).wip_limit, None);
        assert_eq!(set_wip_limit(&c, Some(3)).wip_limit, Some(3));
        assert!(set_is_done(&c, true).is_done);
    }
}
