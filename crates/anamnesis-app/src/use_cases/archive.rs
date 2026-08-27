//! Archiving `is_done` tasks — the scheduled sweep and the manual "Archive
//! all" button, both `docs/DOMAIN.md` §6's one shared function
//! (`anamnesis_core::sweep_done`) called from two different triggers. There
//! is deliberately only one use case here: "the manual path must work even
//! if the scheduled one never fires" means the manual button must be this
//! exact same operation, not a parallel implementation that could drift from
//! it.

use anamnesis_core::policy::Role;
use anamnesis_core::{self as core, TaskId};

use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{BoardQuery, Clock, TaskRepository};

/// Archives every task currently sitting in an `is_done` column. Called by a
/// scheduled sweep ticker (Phase F) on its own schedule, or directly by a
/// user pressing "Archive all" — identical operation either way.
pub async fn archive_done_tasks(
    board: &dyn BoardQuery,
    task_repo: &dyn TaskRepository,
    clock: &dyn Clock,
    role: Option<Role>,
) -> Result<Vec<TaskId>, AppError> {
    if !is_allowed(role, Action::RunArchiveAll) {
        return Err(AppError::Forbidden);
    }
    let board_columns = board.columns_with_tasks().await?;
    let columns: Vec<_> = board_columns.iter().map(|bc| bc.column.clone()).collect();
    let tasks: Vec<_> = board_columns
        .iter()
        .flat_map(|bc| bc.tasks.iter().cloned())
        .collect();

    let now = clock.now();
    let to_archive = core::sweep_done(&tasks, &columns, now);

    let mut archived = Vec::with_capacity(to_archive.len());
    for task_id in to_archive {
        let Some(aggregate) = task_repo.load(task_id).await? else {
            continue; // vanished between the query and now; nothing to do.
        };
        let done = core::archive_task(&aggregate.task, now)?;
        task_repo
            .update(&done, aggregate.task.last_touched_at)
            .await?;
        archived.push(task_id);
    }
    Ok(archived)
}
