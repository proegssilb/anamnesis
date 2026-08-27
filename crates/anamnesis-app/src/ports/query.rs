//! Read-model ports (CQRS-lite) — `docs/DOMAIN.md` §7's "single most
//! important structural addition": the task board is a *query* across
//! everything above the horizon grouped by column, not an aggregate, and
//! global search spans three different entity kinds. Keeping both behind
//! their own ports is what stops board rendering (and search) from loading
//! full object graphs through the per-entity repositories in
//! `crate::ports::repository`.

use async_trait::async_trait;

use anamnesis_core::{
    AreaId, BlockingGraph, BoardState, Column, ColumnId, ProjectId, Tangle, Task, TaskId,
    TaskSummary,
};

use crate::error::RepoError;

/// One item occupying a column slot: a task, or a tangle placed on the
/// board (`docs/DOMAIN.md`'s Tangle section: "untangling is work, so a
/// tangle can be placed on the board... occupying a column slot and
/// counting against that column's WIP limit exactly like a task").
///
/// A heterogeneous enum ordered by `position`, rather than a `Task` list plus
/// a parallel `Tangle` list, is the honest shape: tasks and tangles share one
/// ordering within a column, and a card list is genuinely interleaved by
/// position, not "tasks, then tangles" — a pair of parallel lists cannot
/// express that without the caller re-deriving the interleaving itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardItem {
    Task(Task),
    Tangle(Tangle),
}

impl BoardItem {
    /// This item's board position — `u32::MAX` for the (should-be
    /// unreachable, defensive-only) case of an item this query returned that
    /// somehow is not actually `OnBoard`.
    pub fn position(&self) -> u32 {
        let placement = match self {
            BoardItem::Task(t) => t.placement,
            BoardItem::Tangle(t) => t.placement,
        };
        match placement {
            anamnesis_core::Placement::OnBoard { position, .. } => position,
            anamnesis_core::Placement::Below => u32::MAX,
        }
    }
}

/// One global task-board column, with the tasks and placed tangles currently
/// in it (`docs/DOMAIN.md` §3: "Column *is* status"), interleaved and
/// ordered by [`BoardItem::position`] — the `position` carried in each
/// item's [`anamnesis_core::Placement::OnBoard`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardColumn {
    pub column: Column,
    pub items: Vec<BoardItem>,
}

/// The global task board and the suggestion engine's view of the world —
/// both read-only projections spanning every project and area at once,
/// never owned by any single aggregate (`docs/DOMAIN.md` §7).
#[async_trait]
pub trait BoardQuery: Send + Sync {
    /// Every column, each with the tasks and placed tangles currently in it,
    /// interleaved in position order, in column `position` order. The
    /// whole-board rendering query.
    async fn columns_with_items(&self) -> Result<Vec<BoardColumn>, RepoError>;

    /// How many tasks *and placed tangles* currently sit in `column` — what
    /// a placement move checks against the column's `wip_limit` before it is
    /// allowed to land, and the next open `position` a new item lands at
    /// (`docs/DOMAIN.md`: a tangle "occupies a column slot... exactly like a
    /// task").
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
