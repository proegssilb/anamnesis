//! Task CRUD, placement moves (raising above / dropping below the horizon),
//! checklist parenting, and field values (`docs/DOMAIN.md` §2, §3, §5).
//!
//! Two things this module is specifically responsible for, carried over from
//! the Phase D brief:
//!
//! 1. **The containment-cycle guard is only as strong as its caller.** Core's
//!    `set_parent` takes the new parent's *complete* ancestor chain as a
//!    parameter — it has no repository to query one from. [`set_task_parent`]
//!    walks that chain itself, hop by hop, via [`walk_ancestors`], all the
//!    way to the root, before calling in. A shallow walk (stopping at the
//!    immediate parent) would let a deep cycle through while every core test
//!    still passes — see the Phase D report's dedicated 5-level test.
//! 2. **The column WIP limit is enforced here, at the use-case layer**
//!    (`docs/DOMAIN.md` §7) — `anamnesis_core::move_placement` has no
//!    concept of "how many tasks are already in this column" to check
//!    against.

use std::collections::HashSet;

use anamnesis_core::policy::Role;
use anamnesis_core::{
    self as core, ColumnId, FieldData, FieldDefinition, FieldValue, Placement, ProjectId, Task,
    TaskId,
};

use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{BoardQuery, Clock, IdGen, SearchIndex, TaskAggregate, TaskRepository};

use super::indexing::log_index_failure;

/// Creates a new task, below the horizon, with no parent, then indexes it
/// for global search. See `crate::use_cases::area::create_area`'s doc
/// comment (and `crate::use_cases::indexing`) for why indexing happens here
/// — beside the repository write, in the use case, not in a caller — and
/// what happens if the index write itself fails.
#[allow(clippy::too_many_arguments)]
pub async fn create_task(
    repo: &dyn TaskRepository,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    search: &dyn SearchIndex,
    role: Option<Role>,
    project_id: ProjectId,
    title: &str,
    description: &str,
) -> Result<Task, AppError> {
    if !is_allowed(role, Action::CreateTask) {
        return Err(AppError::Forbidden);
    }
    let task = core::create_task(
        TaskId::new(ids.next()),
        project_id,
        title,
        description,
        clock.now(),
    )?;
    repo.insert(&task).await?;
    if let Err(err) = search.index_task(task.id, task.title.as_str()).await {
        log_index_failure("create_task", err);
    }
    Ok(task)
}

/// Loads a task together with its field values (`docs/DOMAIN.md` §7).
pub async fn view_task(
    repo: &dyn TaskRepository,
    role: Option<Role>,
    id: TaskId,
) -> Result<TaskAggregate, AppError> {
    if !is_allowed(role, Action::ViewTask) {
        return Err(AppError::Forbidden);
    }
    repo.load(id).await?.ok_or(AppError::NotFound)
}

/// Replaces a task's title and description, then re-indexes it — its title,
/// what global search matches against, may have just changed. See
/// [`create_task`]'s doc comment for the indexing-failure policy.
pub async fn edit_task(
    repo: &dyn TaskRepository,
    clock: &dyn Clock,
    search: &dyn SearchIndex,
    role: Option<Role>,
    id: TaskId,
    title: &str,
    description: &str,
) -> Result<Task, AppError> {
    if !is_allowed(role, Action::EditTask) {
        return Err(AppError::Forbidden);
    }
    let aggregate = repo.load(id).await?.ok_or(AppError::NotFound)?;
    let edited = core::edit_task(&aggregate.task, title, description, clock.now())?;
    repo.update(&edited, aggregate.task.last_touched_at).await?;
    if let Err(err) = search.index_task(edited.id, edited.title.as_str()).await {
        log_index_failure("edit_task", err);
    }
    Ok(edited)
}

/// Archives a task, then drops it from the search index —
/// `docs/DOMAIN.md` §2's "vanished from every view unless explicitly
/// searched" does not extend to *plain, unqualified* global search, which
/// has no archived-vs-not-archived toggle of its own; leaving a stale entry
/// indexed would surface an archived task as an ordinary hit. See
/// [`create_task`]'s doc comment for the indexing-failure policy.
pub async fn archive_task(
    repo: &dyn TaskRepository,
    clock: &dyn Clock,
    search: &dyn SearchIndex,
    role: Option<Role>,
    id: TaskId,
) -> Result<Task, AppError> {
    if !is_allowed(role, Action::ArchiveTask) {
        return Err(AppError::Forbidden);
    }
    let aggregate = repo.load(id).await?.ok_or(AppError::NotFound)?;
    let archived = core::archive_task(&aggregate.task, clock.now())?;
    repo.update(&archived, aggregate.task.last_touched_at)
        .await?;
    if let Err(err) = search.remove_task(archived.id).await {
        log_index_failure("archive_task", err);
    }
    Ok(archived)
}

/// Restores an archived task, then re-indexes it — see [`archive_task`] for
/// why it was removed, and [`create_task`]'s doc comment for the
/// indexing-failure policy.
pub async fn unarchive_task(
    repo: &dyn TaskRepository,
    clock: &dyn Clock,
    search: &dyn SearchIndex,
    role: Option<Role>,
    id: TaskId,
) -> Result<Task, AppError> {
    if !is_allowed(role, Action::ArchiveTask) {
        return Err(AppError::Forbidden);
    }
    let aggregate = repo.load(id).await?.ok_or(AppError::NotFound)?;
    let restored = core::unarchive_task(&aggregate.task, clock.now())?;
    repo.update(&restored, aggregate.task.last_touched_at)
        .await?;
    if let Err(err) = search
        .index_task(restored.id, restored.title.as_str())
        .await
    {
        log_index_failure("unarchive_task", err);
    }
    Ok(restored)
}

/// Raises a task onto the global task board at `column`/`position` (from
/// below the horizon, or moving it from another column) — enforcing
/// `column`'s WIP limit first. Reordering *within* the column it is already
/// in never adds to that column's occupancy, so it is exempt from the check
/// even when the column is already full.
pub async fn raise_task(
    task_repo: &dyn TaskRepository,
    board: &dyn BoardQuery,
    clock: &dyn Clock,
    role: Option<Role>,
    task_id: TaskId,
    column: ColumnId,
    position: u32,
) -> Result<Task, AppError> {
    if !is_allowed(role, Action::MoveTaskPlacement) {
        return Err(AppError::Forbidden);
    }
    let aggregate = task_repo.load(task_id).await?.ok_or(AppError::NotFound)?;
    let already_in_column =
        matches!(aggregate.task.placement, Placement::OnBoard { column: c, .. } if c == column);
    if !already_in_column {
        let state = board.board_state(column).await?;
        if let Some(limit) = state.wip_limit
            && state.current_count >= limit
        {
            return Err(AppError::WipLimitExceeded);
        }
    }
    let now = clock.now();
    let moved = core::move_placement(
        &aggregate.task,
        Placement::OnBoard { column, position },
        now,
    )?;
    task_repo
        .update(&moved, aggregate.task.last_touched_at)
        .await?;
    Ok(moved)
}

/// Drops a task back below the horizon, accounting for a bounce
/// (`docs/DOMAIN.md` §5): `left_a_done_column` should be `true` only when
/// the column it is leaving has `is_done: true` (finishing it, not
/// abandoning it) — the caller (which has the `Column`) supplies this, since
/// this use case only has `TaskRepository`.
pub async fn drop_task(
    task_repo: &dyn TaskRepository,
    clock: &dyn Clock,
    role: Option<Role>,
    task_id: TaskId,
    left_a_done_column: bool,
) -> Result<Task, AppError> {
    if !is_allowed(role, Action::MoveTaskPlacement) {
        return Err(AppError::Forbidden);
    }
    let aggregate = task_repo.load(task_id).await?.ok_or(AppError::NotFound)?;
    let bounced = core::bounce_to_below(&aggregate.task, left_a_done_column, clock.now())?;
    task_repo
        .update(&bounced, aggregate.task.last_touched_at)
        .await?;
    Ok(bounced)
}

/// Reorders a task among its checklist siblings.
pub async fn set_checklist_position(
    task_repo: &dyn TaskRepository,
    role: Option<Role>,
    task_id: TaskId,
    position: u32,
) -> Result<Task, AppError> {
    if !is_allowed(role, Action::EditTask) {
        return Err(AppError::Forbidden);
    }
    let aggregate = task_repo.load(task_id).await?.ok_or(AppError::NotFound)?;
    let reordered = core::set_checklist_position(&aggregate.task, position);
    task_repo
        .update(&reordered, aggregate.task.last_touched_at)
        .await?;
    Ok(reordered)
}

/// Sets (or clears) a task's checklist parent, enforcing the acyclic
/// containment rule (`docs/DOMAIN.md` §4) against the **complete** ancestor
/// chain of `new_parent` — see the module doc comment and [`walk_ancestors`].
pub async fn set_task_parent(
    task_repo: &dyn TaskRepository,
    clock: &dyn Clock,
    role: Option<Role>,
    task_id: TaskId,
    new_parent: Option<TaskId>,
) -> Result<Task, AppError> {
    if !is_allowed(role, Action::SetTaskParent) {
        return Err(AppError::Forbidden);
    }
    let aggregate = task_repo.load(task_id).await?.ok_or(AppError::NotFound)?;
    let ancestors = match new_parent {
        None => Vec::new(),
        Some(parent_id) => walk_ancestors(task_repo, parent_id).await?,
    };
    let now = clock.now();
    let updated = core::set_parent(&aggregate.task, new_parent, &ancestors, now)?;
    task_repo
        .update(&updated, aggregate.task.last_touched_at)
        .await?;
    Ok(updated)
}

/// Walks the **complete** ancestor chain of `start` — `start` itself, then
/// its parent, then its parent's parent, and so on until a task with no
/// parent is reached — loading each hop from `task_repo`. Never truncated to
/// just the immediate parent: that is precisely the mistake that would let a
/// containment cycle through at any depth beyond one hop while every
/// existing core test (which only ever exercises what it is handed) keeps
/// passing.
///
/// Defends against an already-corrupted chain that loops on itself (should
/// be unreachable if every write went through [`set_task_parent`], but
/// costs nothing to guard): stops and returns what has been collected so far
/// the moment a task id is seen a second time, rather than looping forever.
async fn walk_ancestors(
    task_repo: &dyn TaskRepository,
    start: TaskId,
) -> Result<Vec<TaskId>, AppError> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut current = Some(start);
    while let Some(id) = current {
        if !seen.insert(id) {
            break;
        }
        chain.push(id);
        current = task_repo
            .load(id)
            .await?
            .and_then(|aggregate| aggregate.task.parent_task_id);
    }
    Ok(chain)
}

/// Sets a task's value for one of its project's field definitions, checking
/// that `data` matches `definition`'s declared kind (`anamnesis_core`'s own
/// rule). `definition` is supplied by the caller (already loaded as part of
/// its owning `ProjectAggregate`) rather than looked up again here, so this
/// use case depends on `TaskRepository` alone.
pub async fn set_task_field_value(
    task_repo: &dyn TaskRepository,
    role: Option<Role>,
    definition: &FieldDefinition,
    task_id: TaskId,
    data: FieldData,
) -> Result<FieldValue, AppError> {
    if !is_allowed(role, Action::SetTaskFieldValue) {
        return Err(AppError::Forbidden);
    }
    task_repo.load(task_id).await?.ok_or(AppError::NotFound)?;
    let value = core::set_field_value(definition, task_id, data)?;
    task_repo.set_field_value(&value).await?;
    Ok(value)
}
