//! Read-model ports (CQRS-lite) — `docs/DOMAIN.md` §7's "single most
//! important structural addition": the task board is a *query* across
//! everything above the horizon grouped by column, not an aggregate, and
//! global search spans three different entity kinds. Keeping both behind
//! their own ports is what stops board rendering (and search) from loading
//! full object graphs through the per-entity repositories in
//! `crate::ports::repository`.

use async_trait::async_trait;

use anamnesis_core::{
    AreaId, BlockingGraph, BoardState, Column, ColumnId, ProjectId, Task, TaskId, TaskSummary,
};

use crate::error::RepoError;

/// One global task-board column, with the tasks currently placed in it
/// (`docs/DOMAIN.md` §3: "Column *is* status"), ordered by
/// [`anamnesis_core::Task::checklist_position`]... no — by board position,
/// i.e. the `position` carried in each task's
/// [`anamnesis_core::Placement::OnBoard`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardColumn {
    pub column: Column,
    pub tasks: Vec<Task>,
}

/// The global task board and the suggestion engine's view of the world —
/// both read-only projections spanning every project and area at once,
/// never owned by any single aggregate (`docs/DOMAIN.md` §7).
#[async_trait]
pub trait BoardQuery: Send + Sync {
    /// Every column, each with the tasks currently placed in it, in column
    /// `position` order. The whole-board rendering query.
    async fn columns_with_tasks(&self) -> Result<Vec<BoardColumn>, RepoError>;

    /// How many tasks currently sit in `column` — what a placement move
    /// checks against the column's `wip_limit` before it is allowed to land.
    async fn count_on_column(&self, column: ColumnId) -> Result<u32, RepoError>;

    /// `column`'s [`BoardState`] (its WIP limit and current occupancy) —
    /// exactly what `anamnesis_core::suggest` needs to size an offer.
    async fn board_state(&self, column: ColumnId) -> Result<BoardState, RepoError>;

    /// Every non-archived task, system-wide, as a [`TaskSummary`] — the
    /// suggestion engine's candidate pool. Eligibility (placement, project
    /// status, blocking, tangles, cooldown) is `anamnesis_core::suggest`'s
    /// job, not this query's: it receives the full candidate set and
    /// decides.
    async fn suggestion_candidates(&self) -> Result<Vec<TaskSummary>, RepoError>;

    /// The system-wide blocking graph plus current tangle membership,
    /// exactly as `anamnesis_core::suggest` needs it (its `graph` parameter).
    async fn blocking_graph(&self) -> Result<BlockingGraph, RepoError>;
}

/// One global-search hit: which kind of entity it is, and enough to render
/// a result line without a follow-up load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchHit {
    Area { id: AreaId, title: String },
    Project { id: ProjectId, title: String },
    Task { id: TaskId, title: String },
}

/// Global search across areas, projects, and tasks (`docs/DOMAIN.md` §8).
/// The read side of the search feature; `crate::ports::SearchIndex` is the
/// write side that keeps it up to date.
#[async_trait]
pub trait SearchQuery: Send + Sync {
    async fn search(&self, text: &str) -> Result<Vec<SearchHit>, RepoError>;
}
