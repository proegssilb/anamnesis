//! Archiving `is_done` tasks — the scheduled sweep and the manual "Archive
//! all" button, both `docs/DOMAIN.md` §6's one shared function
//! (`anamnesis_core::sweep_done`) called from two different triggers. There
//! is deliberately only one use case here: "the manual path must work even
//! if the scheduled one never fires" means the manual button must be this
//! exact same operation, not a parallel implementation that could drift from
//! it.

use anamnesis_core::policy::Role;
use anamnesis_core::{self as core, TangleId, TaskId};

use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{BoardItem, BoardQuery, Clock, SearchIndex, TangleRepository, TaskRepository};

use super::indexing::log_index_failure;

/// What one archive-all/sweep pass actually archived — a task id list plus a
/// tangle id list, kept separate (rather than one merged id list) because
/// they name different entities persisted through different ports and
/// nothing downstream ever wants to treat them interchangeably.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveOutcome {
    pub archived_task_ids: Vec<TaskId>,
    pub archived_tangle_ids: Vec<TangleId>,
}

/// Archives every task currently sitting in an `is_done` column, and every
/// **resolved** tangle sitting in one too (`docs/DOMAIN.md`'s Tangle
/// section: "the archive sweep then treats it like anything else"). Called
/// by a scheduled sweep ticker (Phase F) on its own schedule, or directly by
/// a user pressing "Archive all" — identical operation either way.
///
/// An *unresolved* tangle sitting in a Done column (nothing stops a user
/// from placing one there directly) is left alone: `anamnesis_core::
/// sweep_done_tangles` only ever selects a tangle that is both `OnBoard` in
/// an `is_done` column and already resolved, exactly mirroring
/// `anamnesis_core::sweep_done`'s task-side rule but with the one added
/// resolved-ness condition a `Task` has no equivalent of.
///
/// Each archived task or tangle is dropped from the search index — see
/// `crate::use_cases::task::archive_task`'s doc comment for why, and
/// `crate::use_cases::indexing` for the indexing-failure policy (logged,
/// non-fatal, applied independently per item so one bad index write cannot
/// stop the rest of the sweep from being recorded as archived). Tangles are
/// never indexed for search in the first place (`docs/DOMAIN.md` §8 names
/// areas, projects, and tasks only), so there is no tangle-side call here.
pub async fn archive_done_tasks(
    board: &dyn BoardQuery,
    task_repo: &dyn TaskRepository,
    tangle_repo: &dyn TangleRepository,
    clock: &dyn Clock,
    search: &dyn SearchIndex,
    role: Option<Role>,
) -> Result<ArchiveOutcome, AppError> {
    if !is_allowed(role, Action::RunArchiveAll) {
        return Err(AppError::Forbidden);
    }
    let board_columns = board.columns_with_items().await?;
    let columns: Vec<_> = board_columns.iter().map(|bc| bc.column.clone()).collect();
    let tasks = board_tasks(&board_columns);
    let tangles = board_tangles(&board_columns);

    let now = clock.now();

    Ok(ArchiveOutcome {
        archived_task_ids: archive_swept_tasks(
            task_repo,
            search,
            core::sweep_done(&tasks, &columns, now),
            now,
        )
        .await?,
        archived_tangle_ids: archive_swept_tangles(
            tangle_repo,
            core::sweep_done_tangles(&tangles, &columns, now),
            now,
        )
        .await?,
    })
}

/// Every task currently on the board, flattened out of its column. The two
/// kinds share one item list per column (`docs/DOMAIN.md`'s Tangle section),
/// so reading either kind means filtering the other out.
fn board_tasks(board_columns: &[crate::ports::BoardColumn]) -> Vec<core::Task> {
    board_columns
        .iter()
        .flat_map(|bc| bc.items.iter())
        .filter_map(|item| match item {
            BoardItem::Task(t) => Some(t.clone()),
            BoardItem::Tangle(_) => None,
        })
        .collect()
}

/// Every tangle currently placed on the board — the counterpart of
/// [`board_tasks`].
fn board_tangles(board_columns: &[crate::ports::BoardColumn]) -> Vec<core::Tangle> {
    board_columns
        .iter()
        .flat_map(|bc| bc.items.iter())
        .filter_map(|item| match item {
            BoardItem::Tangle(t) => Some(t.clone()),
            BoardItem::Task(_) => None,
        })
        .collect()
}

/// Archives each swept task and drops it from the search index, returning
/// the ids actually archived. A task that vanished between the board query
/// and now is skipped rather than failing the pass; an index write that
/// fails is logged and skipped for the same reason (see this module's
/// `archive_done_tasks` doc comment).
async fn archive_swept_tasks(
    task_repo: &dyn TaskRepository,
    search: &dyn SearchIndex,
    to_archive: Vec<TaskId>,
    now: anamnesis_core::Timestamp,
) -> Result<Vec<TaskId>, AppError> {
    let mut archived = Vec::with_capacity(to_archive.len());
    for task_id in to_archive {
        let Some(aggregate) = task_repo.load(task_id).await? else {
            continue; // vanished between the query and now; nothing to do.
        };
        let done = core::archive_task(&aggregate.task, now)?;
        task_repo
            .update(&done, aggregate.task.last_touched_at)
            .await?;
        if let Err(err) = search.remove_task(task_id).await {
            log_index_failure("archive_done_tasks", err);
        }
        archived.push(task_id);
    }
    Ok(archived)
}

/// Archives each swept tangle, returning the ids actually archived. Tangles
/// are never search-indexed, so unlike [`archive_swept_tasks`] there is no
/// index write to undo.
async fn archive_swept_tangles(
    tangle_repo: &dyn TangleRepository,
    to_archive: Vec<TangleId>,
    now: anamnesis_core::Timestamp,
) -> Result<Vec<TangleId>, AppError> {
    let mut archived = Vec::with_capacity(to_archive.len());
    for tangle_id in to_archive {
        let Some(tangle) = tangle_repo.load(tangle_id).await? else {
            continue; // vanished between the query and now; nothing to do.
        };
        let done = core::archive_tangle(&tangle, now)?;
        tangle_repo.update(&done).await?;
        archived.push(tangle_id);
    }
    Ok(archived)
}
