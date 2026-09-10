//! Repository ports for project sync config and task/issue links (issues
//! #40, #41).

use async_trait::async_trait;

use anamnesis_core::{ProjectId, TaskId, Timestamp};

use crate::error::RepoError;
use crate::sync::{ProjectSyncConfig, TaskSyncLink};

/// Loads and writes [`ProjectSyncConfig`]s — one per project, like
/// [`crate::ports::SettingsRepository`] is one globally.
#[async_trait]
pub trait ProjectSyncConfigRepository: Send + Sync {
    async fn load(&self, project_id: ProjectId) -> Result<Option<ProjectSyncConfig>, RepoError>;
    /// Every config with `enabled: true` — the reconciliation ticker's
    /// worklist for one tick.
    async fn list_enabled(&self) -> Result<Vec<ProjectSyncConfig>, RepoError>;
    /// At most one row per project, so insert and update collapse into one
    /// upsert — the config form always posts the full state.
    async fn upsert(&self, config: &ProjectSyncConfig) -> Result<(), RepoError>;
    /// Stamps `last_synced_at` and `last_sync_error`, and only those two
    /// fields — the reconciliation pass's one write, isolated from
    /// [`Self::upsert`] for the same reason
    /// `SettingsRepository::record_sweep` is isolated from `update`: the
    /// ticker and an admin editing the config through the UI can run
    /// concurrently, and a whole-row `upsert` would let one clobber the
    /// other's just-written value.
    async fn record_sync_result(
        &self,
        project_id: ProjectId,
        synced_at: Timestamp,
        error: Option<&str>,
    ) -> Result<(), RepoError>;
    async fn delete(&self, project_id: ProjectId) -> Result<(), RepoError>;
}

/// Loads and writes [`TaskSyncLink`]s. No optimistic-concurrency check on
/// `update`, unlike `TaskRepository::update` — only the lease-guarded
/// reconciliation pass ever writes this table, so there is no concurrent
/// editor to race.
#[async_trait]
pub trait TaskSyncLinkRepository: Send + Sync {
    async fn load_by_task(&self, task_id: TaskId) -> Result<Option<TaskSyncLink>, RepoError>;
    async fn load_by_issue(
        &self,
        project_id: ProjectId,
        external_issue_number: u64,
    ) -> Result<Option<TaskSyncLink>, RepoError>;
    async fn list_for_project(&self, project_id: ProjectId) -> Result<Vec<TaskSyncLink>, RepoError>;
    async fn insert(&self, link: &TaskSyncLink) -> Result<(), RepoError>;
    async fn update(&self, link: &TaskSyncLink) -> Result<(), RepoError>;
}
