//! Repositioning a card on the global task board (`docs/DOMAIN.md` §8:
//! "Sortable drags, htmx persists") — the one use case shared by the htmx
//! drag path and its plain-form fallback, since both post to the same
//! endpoint and neither cares how the client computed its inputs.
//!
//! **Tasks and tangles share one code path**, exactly as
//! `crate::ports::BoardItem` unifies them for reading: [`BoardItemKind`]
//! names which one is being moved, and [`reposition_board_item`] treats both
//! uniformly from there.
//!
//! **Only the dragged item's own move is permission-gated.** Moving it can
//! displace every other item already in its destination column (and, if it
//! changed columns, close the gap it left behind) — those position-only
//! rewrites are system bookkeeping to keep `docs/DOMAIN.md` §7's "positions
//! never collide" invariant true after a genuine mid-column insertion, not a
//! substantive edit to *those* items, so they carry no separate role check.
//! This is the same reasoning `crate::use_cases::tangle`'s module doc
//! comment already gives for tangle detection's own system-wide rewrites.

use anamnesis_core::policy::Role;
use anamnesis_core::{self as core, ColumnId, Placement, TangleId, TaskId};

use super::recurring::{DoneColumns, record_recurring_completion};
use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{
    BoardColumn, BoardItem, BoardQuery, Clock, TangleRepository, TaskRepository, TimezoneResolver,
};

/// Which kind of board item [`reposition_board_item`] is moving — the two
/// variants of [`BoardItem`], but carrying just the id a caller already has
/// (from a form field or a drag event), not a freshly loaded value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardItemKind {
    Task(anamnesis_core::TaskId),
    Tangle(anamnesis_core::TangleId),
}

fn identity_of(item: &BoardItem) -> BoardItemKind {
    match item {
        BoardItem::Task(t) => BoardItemKind::Task(t.id),
        BoardItem::Tangle(t) => BoardItemKind::Tangle(t.id),
    }
}

/// Moves `item` to `column`/`position` and renumbers every sibling this
/// displaces so the column's positions stay contiguous — the drag-and-drop
/// (and plain-form) reposition endpoint's whole job.
///
/// `position` is clamped to the destination column's length after `item` is
/// removed from consideration, so an out-of-range value (a stale client, a
/// hand-edited form) lands at the nearest valid end rather than erroring.
/// The WIP limit is checked only when `item` is not already in `column`
/// (reordering within a column already at its limit must still work,
/// exactly like `crate::use_cases::task::raise_task`'s own exemption).
///
/// Dragging a recurring task into a done column from outside one completes
/// it, exactly as raising it there does (`crate::use_cases::recurring`);
/// `tz`/`zone` give the local calendar that is counted in.
#[allow(clippy::too_many_arguments)]
pub async fn reposition_board_item(
    task_repo: &dyn TaskRepository,
    tangle_repo: &dyn TangleRepository,
    board: &dyn BoardQuery,
    clock: &dyn Clock,
    tz: &dyn TimezoneResolver,
    zone: &str,
    role: Option<Role>,
    item: BoardItemKind,
    column: ColumnId,
    position: u32,
) -> Result<(), AppError> {
    let action = match item {
        BoardItemKind::Task(_) => Action::MoveTaskPlacement,
        BoardItemKind::Tangle(_) => Action::PlaceTangle,
    };
    if !is_allowed(role, action) {
        return Err(AppError::Forbidden);
    }

    let columns = board.columns_with_items().await?;
    let (prev_column, already_in_dest, dest) = plan_column_order(&columns, item, column, position);
    check_wip_limit(board, column, already_in_dest).await?;
    renumber_column(task_repo, tangle_repo, clock, dest, column).await?;
    if let Some((prev, src)) = plan_source_order(&columns, item, prev_column, column) {
        renumber_column(task_repo, tangle_repo, clock, src, prev).await?;
    }

    let entered_done = DoneColumns::from_board(&columns).entered(prev_column, column);
    complete_if_entered_done(task_repo, clock, tz, zone, item, entered_done).await
}

/// The order the column `item` left behind should close up to, if it came
/// from a column other than the destination — the source-side counterpart
/// of [`plan_column_order`].
fn plan_source_order(
    columns: &[BoardColumn],
    item: BoardItemKind,
    prev_column: Option<ColumnId>,
    column: ColumnId,
) -> Option<(ColumnId, Vec<BoardItemKind>)> {
    let prev = prev_column.filter(|prev| *prev != column)?;
    let src = columns
        .iter()
        .find(|bc| bc.column.id == prev)
        .map(|bc| {
            bc.items
                .iter()
                .map(identity_of)
                .filter(|i| *i != item)
                .collect()
        })
        .unwrap_or_default();
    Some((prev, src))
}

/// Stamps a completion on the dragged `item` if it is a recurring task and
/// the move `entered_done` (a done column, from outside one). Tangles have
/// no recurrence.
async fn complete_if_entered_done(
    task_repo: &dyn TaskRepository,
    clock: &dyn Clock,
    tz: &dyn TimezoneResolver,
    zone: &str,
    item: BoardItemKind,
    entered_done: bool,
) -> Result<(), AppError> {
    let BoardItemKind::Task(id) = item else {
        return Ok(());
    };
    if !entered_done {
        return Ok(());
    }
    let aggregate = task_repo.load(id).await?.ok_or(AppError::NotFound)?;
    if aggregate.task.recurrence.is_none() {
        return Ok(());
    }
    let completed = record_recurring_completion(&aggregate.task, clock.now(), tz, zone)?;
    task_repo
        .update(&completed, aggregate.task.last_touched_at)
        .await?;
    Ok(())
}

/// Scans `columns` for `item`'s current placement and computes the
/// destination column's final item order after moving `item` to `position`
/// — the pure planning step [`reposition_board_item`] then just carries out.
/// `position` is clamped to the destination's length (after `item` is
/// removed from consideration), so an out-of-range value (a stale client, a
/// hand-edited form) lands at the nearest valid end rather than erroring.
///
/// Returns `(item's source column, if any and other than the destination;
/// whether item was already in the destination column; the destination's
/// final order)`. The middle value is what [`check_wip_limit`] needs — it
/// must be read from the *original* order, before `item` is inserted, since
/// after insertion the destination always contains it.
fn plan_column_order(
    columns: &[BoardColumn],
    item: BoardItemKind,
    column: ColumnId,
    position: u32,
) -> (Option<ColumnId>, bool, Vec<BoardItemKind>) {
    let mut prev_column: Option<ColumnId> = None;
    let mut dest: Vec<BoardItemKind> = Vec::new();
    for bc in columns {
        let ids: Vec<BoardItemKind> = bc.items.iter().map(identity_of).collect();
        if ids.contains(&item) {
            prev_column = Some(bc.column.id);
        }
        if bc.column.id == column {
            dest = ids;
        }
    }

    let already_in_dest = dest.contains(&item);
    dest.retain(|i| *i != item);
    let clamped = (position as usize).min(dest.len());
    dest.insert(clamped, item);

    (prev_column, already_in_dest, dest)
}

/// Rejects the move if `column` is at its WIP limit and the item would be a
/// genuine new arrival there. Skipped entirely when `already_in_dest` —
/// reordering within a column already at its limit must still work, exactly
/// like `crate::use_cases::task::raise_task`'s own exemption, since it does
/// not change `current_count`. Shared by both use cases for exactly that
/// reason.
pub(crate) async fn check_wip_limit(
    board: &dyn BoardQuery,
    column: ColumnId,
    already_in_dest: bool,
) -> Result<(), AppError> {
    if already_in_dest {
        return Ok(());
    }
    let state = board.board_state(column).await?;
    if let Some(limit) = state.wip_limit
        && state.current_count >= limit
    {
        return Err(AppError::WipLimitExceeded);
    }
    Ok(())
}

/// Writes out `items`' new order in `column` — shared by
/// [`reposition_board_item`]'s destination-column write and its
/// source-column renumber, which were previously the same three lines
/// copy-pasted twice.
async fn renumber_column(
    task_repo: &dyn TaskRepository,
    tangle_repo: &dyn TangleRepository,
    clock: &dyn Clock,
    items: Vec<BoardItemKind>,
    column: ColumnId,
) -> Result<(), AppError> {
    for (idx, ident) in items.into_iter().enumerate() {
        write_position(task_repo, tangle_repo, clock, ident, column, idx as u32).await?;
    }
    Ok(())
}

/// Writes a task's placement to `target` if it is not already exactly there.
async fn write_task_position(
    task_repo: &dyn TaskRepository,
    clock: &dyn Clock,
    id: TaskId,
    target: Placement,
) -> Result<(), AppError> {
    let aggregate = task_repo.load(id).await?.ok_or(AppError::NotFound)?;
    if aggregate.task.placement == target {
        return Ok(());
    }
    let moved = core::move_placement(&aggregate.task, target, clock.now())?;
    task_repo
        .update(&moved, aggregate.task.last_touched_at)
        .await?;
    Ok(())
}

/// Writes a tangle's placement to `column`/`position` if it is not already
/// exactly there.
async fn write_tangle_position(
    tangle_repo: &dyn TangleRepository,
    id: TangleId,
    column: ColumnId,
    position: u32,
    target: Placement,
) -> Result<(), AppError> {
    let tangle = tangle_repo.load(id).await?.ok_or(AppError::NotFound)?;
    if tangle.placement == target {
        return Ok(());
    }
    let moved = core::place_tangle(&tangle, column, position)?;
    tangle_repo.update(&moved).await?;
    Ok(())
}

/// Writes `item`'s placement to `column`/`position` if it is not already
/// exactly there — the no-op skip is what keeps [`reposition_board_item`]
/// from re-touching (and re-stamping `last_touched_at` on) every untouched
/// sibling on every drag, not just the ones that actually moved.
async fn write_position(
    task_repo: &dyn TaskRepository,
    tangle_repo: &dyn TangleRepository,
    clock: &dyn Clock,
    item: BoardItemKind,
    column: ColumnId,
    position: u32,
) -> Result<(), AppError> {
    let target = Placement::OnBoard { column, position };
    match item {
        BoardItemKind::Task(id) => write_task_position(task_repo, clock, id, target).await,
        BoardItemKind::Tangle(id) => {
            write_tangle_position(tangle_repo, id, column, position, target).await
        }
    }
}
