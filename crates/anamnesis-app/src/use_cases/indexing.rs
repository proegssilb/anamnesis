//! Shared policy for keeping [`crate::ports::SearchIndex`] in step with a
//! write that already succeeded through a repository port.
//!
//! **Indexing runs beside the repository write, in the use case, not in
//! `anamnesis-web`'s handlers.** Handlers are transport: any caller of these
//! use cases — the web UI today, a future MCP server or CLI or JSON API per
//! `docs/CONTEXT.md`'s "no MCP just yet" — must get a consistent index
//! without having to remember to update it itself. This module exists so
//! every use case that touches an indexable entity (area/project/task)
//! shares one place to decide what happens when the index write itself
//! fails.
//!
//! **Decision: an index-write failure is logged and non-fatal.** The search
//! index is derived, rebuildable data (`docs/DOMAIN.md` §7's `SearchIndex`
//! doc comment; it exists purely to keep `SearchQuery` current). By the time
//! [`log_index_failure`] is called, the entity's own repository write has
//! already committed — the user's create/edit/archive succeeded and must not
//! be rolled back or reported as a failure just because the *index* fell out
//! of step. So a use case calls the index port, and on `Err` logs and
//! continues rather than propagating an `AppError` that would tell the
//! caller their write failed when it didn't. The cost is an accepted,
//! documented one: a title can transiently go missing (or stale) from search
//! results until the next successful write to that entity, or a future
//! reindex sweep. `anamnesis-app` depends on nothing but `anamnesis-core`,
//! `async-trait`, and value crates (no logging crate among them), so
//! `eprintln!` — plain `std`, not a dependency — is the only channel
//! available at this layer; `anamnesis-web`'s adapters have `tracing` for
//! anything richer.
pub(crate) fn log_index_failure(operation: &str, err: crate::error::RepoError) {
    eprintln!("anamnesis: search index update failed during {operation}: {err}");
}

use crate::error::AppError;
use crate::ports::{AreaRepository, ProjectRepository, SearchIndex, TaskRepository};

/// What one reindex pass touched, broken out per entity kind since nothing
/// downstream treats them interchangeably (mirrors
/// `crate::use_cases::archive::ArchiveOutcome`'s own per-kind split).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReindexOutcome {
    pub areas: usize,
    pub projects: usize,
    pub tasks: usize,
}

/// Rebuilds [`SearchIndex`] from scratch by re-asserting every current
/// area's, project's, and task's indexed state.
///
/// **Why this is a real "from scratch" rebuild without a hard table clear.**
/// `index_*`/`remove_*` are the only writers of the underlying search store,
/// and nothing in this domain model ever hard-deletes an `Area`, `Project`,
/// or `Task` (only archives them) — so every row a real adapter's search
/// table holds corresponds to some entity this sweep will visit, and
/// `index_*` unconditionally overwrites whatever was there. Re-asserting
/// every current entity's state therefore converges to the same result a
/// clear-then-rebuild would, without ever leaving search briefly empty
/// mid-sweep.
///
/// **Why an archived entity is `index_*`-then-`remove_*`, never `remove_*`
/// alone.** `remove_*` only flags an *existing* row archived — on both real
/// adapters it is an `UPDATE ... WHERE entity_kind = ... AND entity_id = ...`,
/// which matches zero rows and writes nothing if that entity was never
/// indexed in the first place. That gap is exactly the case this sweep must
/// close: an entity whose original `index_*` call failed (logged and
/// non-fatal, per this module's own policy above) and was *then* archived
/// would have called `remove_*` on a row that never existed, leaving it
/// permanently unindexed — invisible to both ordinary and archived search,
/// forever, with nothing left to notice. Calling `index_*` first guarantees
/// the row exists (creating it if this is exactly that gap, harmlessly
/// re-writing it if not) before `remove_*` flips it archived.
///
/// Called only by the scheduled ticker (`anamnesis-web` spawns it in
/// `main.rs`), never from a request path, so — like
/// `crate::use_cases::expire_stale_uploads` — this takes no `Role` and
/// enforces no permission.
pub async fn reindex_all(
    areas: &dyn AreaRepository,
    projects: &dyn ProjectRepository,
    tasks: &dyn TaskRepository,
    search: &dyn SearchIndex,
) -> Result<ReindexOutcome, AppError> {
    Ok(ReindexOutcome {
        areas: reindex_areas(areas, search).await?,
        projects: reindex_projects(projects, search).await?,
        tasks: reindex_tasks(tasks, search).await?,
    })
}

/// `Area` has no `archived_at` (`docs/DOMAIN.md` §3), so every area is
/// always re-asserted via `index_area`, never `remove_area`.
async fn reindex_areas(
    repo: &dyn AreaRepository,
    search: &dyn SearchIndex,
) -> Result<usize, AppError> {
    let areas = repo.list().await?;
    for area in &areas {
        if let Err(err) = search.index_area(area.id, area.title.as_str()).await {
            log_index_failure("reindex_sweep(area)", err);
        }
    }
    Ok(areas.len())
}

async fn reindex_projects(
    repo: &dyn ProjectRepository,
    search: &dyn SearchIndex,
) -> Result<usize, AppError> {
    let projects = repo.list_all().await?;
    for project in &projects {
        reindex_one_project(project, search).await;
    }
    Ok(projects.len())
}

/// `index_project`, then `remove_project` if archived — see `reindex_all`'s
/// doc comment for why archiving alone is not enough to guarantee the row
/// exists at all.
async fn reindex_one_project(project: &anamnesis_core::Project, search: &dyn SearchIndex) {
    if let Err(err) = search
        .index_project(project.id, project.title.as_str())
        .await
    {
        log_index_failure("reindex_sweep(project)", err);
    }
    if project.archived_at.is_some()
        && let Err(err) = search.remove_project(project.id).await
    {
        log_index_failure("reindex_sweep(project)", err);
    }
}

async fn reindex_tasks(
    repo: &dyn TaskRepository,
    search: &dyn SearchIndex,
) -> Result<usize, AppError> {
    let tasks = repo.list_all().await?;
    for task in &tasks {
        reindex_one_task(task, search).await;
    }
    Ok(tasks.len())
}

/// `index_task`, then `remove_task` if archived — see `reindex_all`'s doc
/// comment for why archiving alone is not enough to guarantee the row
/// exists at all.
async fn reindex_one_task(task: &anamnesis_core::Task, search: &dyn SearchIndex) {
    if let Err(err) = search.index_task(task.id, task.title.as_str()).await {
        log_index_failure("reindex_sweep(task)", err);
    }
    if task.archived_at.is_some()
        && let Err(err) = search.remove_task(task.id).await
    {
        log_index_failure("reindex_sweep(task)", err);
    }
}
