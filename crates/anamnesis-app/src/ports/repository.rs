//! Per-entity repository ports for the real domain model
//! (`docs/DOMAIN.md` §7: "Repository ports become per-entity with targeted
//! operations" — whole-aggregate save is dead).
//!
//! Aggregates match the table in `docs/DOMAIN.md` §7:
//! - `Area` — tiny, no children.
//! - `Project` — loaded with its field definitions and relationship kinds
//!   ([`ProjectAggregate`]).
//! - `Task` — loaded with its field values ([`TaskAggregate`]); comments and
//!   attachments are paged separately, hence their own repositories.
//! - `Relationship`, `Tangle` — standalone, system-derived.
//! - `Comment`, `Attachment` — append-heavy, paged.

use async_trait::async_trait;

use anamnesis_core::{
    Area, AreaId, FieldDefinition, FieldValue, KindId, Project, ProjectId, Relationship,
    RelationshipId, RelationshipKind, Tangle, Task, TaskId, Timestamp,
};

use crate::entities::{Attachment, AttachmentId, Comment, CommentId};
use crate::error::RepoError;

/// Loads, lists, and writes [`Area`]s. Tiny aggregate, no children, no
/// archival (`docs/DOMAIN.md` §3 gives `Area` no `archived_at`).
#[async_trait]
pub trait AreaRepository: Send + Sync {
    async fn load(&self, id: AreaId) -> Result<Option<Area>, RepoError>;
    /// Every area, ordered by `position`.
    async fn list(&self) -> Result<Vec<Area>, RepoError>;
    async fn insert(&self, area: &Area) -> Result<(), RepoError>;
    async fn update(&self, area: &Area) -> Result<(), RepoError>;
}

/// A [`Project`] loaded with its field definitions and relationship kinds —
/// "config-sized", per `docs/DOMAIN.md` §7, so loading all three together on
/// every read is the right default (contrast with `Task`'s comments and
/// attachments, which are paged separately instead).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectAggregate {
    pub project: Project,
    pub field_definitions: Vec<FieldDefinition>,
    pub relationship_kinds: Vec<RelationshipKind>,
}

/// Loads, lists, and writes [`Project`]s, their [`FieldDefinition`]s, and
/// their project-local [`RelationshipKind`]s.
#[async_trait]
pub trait ProjectRepository: Send + Sync {
    async fn load(&self, id: ProjectId) -> Result<Option<ProjectAggregate>, RepoError>;
    async fn list_by_area(&self, area_id: AreaId) -> Result<Vec<Project>, RepoError>;
    /// How many projects currently hold `status == Active`, excluding
    /// `excluding` itself if given — exactly the count
    /// [`anamnesis_core::transition_status`] needs to enforce
    /// `Settings.active_project_limit` (`docs/DOMAIN.md` §3).
    async fn count_active(&self, excluding: Option<ProjectId>) -> Result<u32, RepoError>;
    async fn insert(&self, project: &Project) -> Result<(), RepoError>;
    async fn update(&self, project: &Project) -> Result<(), RepoError>;
    async fn insert_field_definition(&self, definition: &FieldDefinition)
    -> Result<(), RepoError>;
    async fn update_field_definition(&self, definition: &FieldDefinition)
    -> Result<(), RepoError>;
    async fn insert_relationship_kind(&self, kind: &RelationshipKind) -> Result<(), RepoError>;
    /// Looks up a *project-local, custom* relationship kind by id. Built-in
    /// kinds (`docs/DOMAIN.md` §3: `blocks`, `relates to`, `duplicates`) are
    /// fixed constants (`anamnesis_core::builtin_blocks` and friends) and are
    /// never stored here — a caller resolving a [`KindId`] of unknown
    /// provenance should check the three well-known built-in ids first (see
    /// `crate::use_cases::relationship::resolve_kind`) and fall back to this
    /// method only if none matched.
    async fn load_relationship_kind(&self, id: KindId)
    -> Result<Option<RelationshipKind>, RepoError>;
}

/// A [`Task`] loaded with its field values. Comments and attachments load
/// separately through their own repositories (`docs/DOMAIN.md` §3, §7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAggregate {
    pub task: Task,
    pub field_values: Vec<FieldValue>,
}

/// Every way [`TaskRepository::update`] can fail: either the world failed
/// ([`RepoError`]), or the optimistic-concurrency check itself failed.
#[derive(Debug, thiserror::Error)]
pub enum TaskUpdateError {
    /// `expected_last_touched_at` no longer matches the task's stored
    /// `last_touched_at` — someone else wrote to this task first
    /// (`docs/DOMAIN.md` §7: "last-write-wins is no longer acceptable").
    /// Nothing was written.
    #[error("task was concurrently modified")]
    Conflict,
    #[error(transparent)]
    Repo(#[from] RepoError),
}

impl PartialEq for TaskUpdateError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (TaskUpdateError::Conflict, TaskUpdateError::Conflict) => true,
            (TaskUpdateError::Repo(a), TaskUpdateError::Repo(b)) => a == b,
            _ => false,
        }
    }
}

/// Loads, lists, and writes [`Task`]s and their [`FieldValue`]s.
///
/// `update` is the one optimistic-concurrency-checked write in this crate
/// (`docs/DOMAIN.md` §7). `insert` carries no such check — there is nothing
/// to conflict with yet.
#[async_trait]
pub trait TaskRepository: Send + Sync {
    async fn load(&self, id: TaskId) -> Result<Option<TaskAggregate>, RepoError>;
    /// A task's immediate checklist children (`parent_task_id == id`), not
    /// the full subtree.
    async fn list_children(&self, parent_id: TaskId) -> Result<Vec<Task>, RepoError>;
    async fn insert(&self, task: &Task) -> Result<(), RepoError>;
    /// Writes `task`, but only if the stored row's `last_touched_at` still
    /// equals `expected_last_touched_at` — the value the caller read `task`
    /// with before computing its edit. A mismatch means someone else wrote
    /// to this task in between, and returns
    /// [`TaskUpdateError::Conflict`] having written nothing.
    async fn update(
        &self,
        task: &Task,
        expected_last_touched_at: Timestamp,
    ) -> Result<(), TaskUpdateError>;
    async fn set_field_value(&self, value: &FieldValue) -> Result<(), RepoError>;
}

/// Loads, lists, and writes [`Relationship`] edges and looks up the
/// system-wide `blocks` subgraph tangle detection needs.
///
/// Edges live outside any project (`docs/DOMAIN.md` §3), so this port has no
/// project-scoping parameter anywhere.
#[async_trait]
pub trait RelationshipRepository: Send + Sync {
    async fn load(&self, id: RelationshipId) -> Result<Option<Relationship>, RepoError>;
    async fn list_for_task(&self, task_id: TaskId) -> Result<Vec<Relationship>, RepoError>;
    /// Every relationship using the built-in `blocks` kind, system-wide —
    /// exactly the edge set `anamnesis_core::detect_tangles` needs.
    async fn list_blocking(&self) -> Result<Vec<Relationship>, RepoError>;
    async fn insert(&self, relationship: &Relationship) -> Result<(), RepoError>;
    async fn delete(&self, id: RelationshipId) -> Result<(), RepoError>;
}

/// Loads and writes [`Tangle`]s — system-derived, reconciled against fresh
/// detection passes by the use case in `crate::use_cases::tangle`, never
/// edited by hand.
#[async_trait]
pub trait TangleRepository: Send + Sync {
    /// Every tangle with `resolved_at: None` — the `previous` input
    /// `anamnesis_core::reconcile` needs.
    async fn list_active(&self) -> Result<Vec<Tangle>, RepoError>;
    async fn insert(&self, tangle: &Tangle) -> Result<(), RepoError>;
    /// Persists a tangle whose `resolved_at` has just been stamped (or,
    /// defensively, any other field `reconcile` may have changed on an
    /// already-stored tangle).
    async fn update(&self, tangle: &Tangle) -> Result<(), RepoError>;
}

/// Loads, lists, and writes [`Comment`]s, paged per task
/// (`docs/DOMAIN.md` §3: "append-heavy, rarely all needed at once").
#[async_trait]
pub trait CommentRepository: Send + Sync {
    async fn list_for_task(&self, task_id: TaskId) -> Result<Vec<Comment>, RepoError>;
    async fn load(&self, id: CommentId) -> Result<Option<Comment>, RepoError>;
    async fn insert(&self, comment: &Comment) -> Result<(), RepoError>;
    async fn update(&self, comment: &Comment) -> Result<(), RepoError>;
    async fn delete(&self, id: CommentId) -> Result<(), RepoError>;
}

/// Loads, lists, and writes [`Attachment`]s, paged per task.
#[async_trait]
pub trait AttachmentRepository: Send + Sync {
    async fn list_for_task(&self, task_id: TaskId) -> Result<Vec<Attachment>, RepoError>;
    async fn load(&self, id: AttachmentId) -> Result<Option<Attachment>, RepoError>;
    async fn insert(&self, attachment: &Attachment) -> Result<(), RepoError>;
    async fn delete(&self, id: AttachmentId) -> Result<(), RepoError>;
}
