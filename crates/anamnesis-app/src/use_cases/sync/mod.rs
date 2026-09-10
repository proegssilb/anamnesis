//! Configuring a project's external-issue-tracker sync and running one
//! reconciliation pass against it (issues #40, #41). Polling, not webhooks
//! — see `docs/DOMAIN.md`'s sync section for why: this app has no
//! public-ingress infrastructure, and a self-hosted deployment may have no
//! reachable public URL at all.
//!
//! This module owns the config CRUD ([`view_sync_status`],
//! [`configure_or_update_project_sync`]) and the one entry point both
//! `anamnesis-web`'s ticker and its "Sync now" button call
//! ([`run_and_record`]). The reconciliation algorithm itself —
//! `reconcile_project` and its five steps — lives in [`reconcile`], split
//! out purely because "manage a project's stored config" and "run one sync
//! pass against it" are different concerns that happened to share
//! [`ProjectSyncConfig`]/[`SyncPorts`], not to dodge a line-count rule.

mod reconcile;

use anamnesis_core::policy::Role;
use anamnesis_core::{ProjectId, Timestamp};

use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{
    BoardQuery, Clock, CommentRepository, IdGen, IssueTrackerClient, ProjectSyncConfigRepository,
    SearchIndex, TaskRepository, TaskSyncLinkRepository,
};
use crate::sync::{
    ProjectSyncConfig, SyncConfigFields, configure_project_sync, edit_project_sync_config,
    rotate_project_sync_token,
};

use reconcile::reconcile_project;

/// Everything one reconciliation pass over a single project needs from the
/// world. Eight ports, every one independently used in the body — a real
/// dependency list for one operation, not a parameter-count workaround.
pub struct SyncPorts<'a> {
    pub configs: &'a dyn ProjectSyncConfigRepository,
    pub links: &'a dyn TaskSyncLinkRepository,
    pub tasks: &'a dyn TaskRepository,
    pub comments: &'a dyn CommentRepository,
    /// Needed for exactly one check: whether a not-yet-linked task already
    /// sits in an `is_done` board column, so step 1 doesn't push it as a new
    /// issue only to close it moments later. Not used for anything else here —
    /// reconciliation never renders or reorders the board.
    pub board: &'a dyn BoardQuery,
    pub search: &'a dyn SearchIndex,
    pub clock: &'a dyn Clock,
    pub ids: &'a dyn IdGen,
}

/// What one reconciliation pass actually did, for logging and the sync
/// status line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    pub pushed_new_task_count: usize,
    pub imported_task_count: usize,
    pub pushed_to_remote_count: usize,
    pub pulled_from_remote_count: usize,
    pub imported_comment_count: usize,
}

/// Loads the current sync config for a project. `Err(AppError::Forbidden)`
/// for anyone but a Project/System Admin.
pub async fn view_sync_status(
    repo: &dyn ProjectSyncConfigRepository,
    role: Option<Role>,
    project_id: ProjectId,
) -> Result<Option<ProjectSyncConfig>, AppError> {
    if !is_allowed(role, Action::ManageProjectSync) {
        return Err(AppError::Forbidden);
    }
    Ok(repo.load(project_id).await?)
}

/// Creates or replaces a project's sync config — the settings form always
/// posts the full state, so insert and update collapse into one upsert, same
/// as `crate::use_cases::update_settings` does for the one global settings
/// row. `encrypted_token` is whatever the caller has already decided should
/// end up stored (freshly encrypted from a submitted token, or the existing
/// ciphertext re-supplied when the form's token field was left blank) — this
/// use case does not decide that, only persists it.
pub async fn configure_or_update_project_sync(
    repo: &dyn ProjectSyncConfigRepository,
    clock: &dyn Clock,
    role: Option<Role>,
    project_id: ProjectId,
    fields: SyncConfigFields<'_>,
    encrypted_token: Vec<u8>,
    enabled: bool,
) -> Result<ProjectSyncConfig, AppError> {
    if !is_allowed(role, Action::ManageProjectSync) {
        return Err(AppError::Forbidden);
    }
    let now = clock.now();
    let existing = repo.load(project_id).await?;
    let config =
        build_project_sync_config(existing, project_id, fields, encrypted_token, enabled, now)?;
    repo.upsert(&config).await?;
    Ok(config)
}

/// Decides what the stored config should become: a fresh one via
/// [`configure_project_sync`] when none exists yet, or the existing one
/// edited via [`edit_project_sync_config`] plus [`rotate_project_sync_token`]
/// otherwise. Pure (no port access) and split out from
/// [`configure_or_update_project_sync`] specifically so that async use case
/// stays a short, obvious "load, build, save" shape rather than growing this
/// decision inline.
fn build_project_sync_config(
    existing: Option<ProjectSyncConfig>,
    project_id: ProjectId,
    fields: SyncConfigFields<'_>,
    encrypted_token: Vec<u8>,
    enabled: bool,
    now: Timestamp,
) -> Result<ProjectSyncConfig, AppError> {
    match existing {
        None => {
            let created = configure_project_sync(project_id, fields, encrypted_token, now)?;
            Ok(ProjectSyncConfig { enabled, ..created })
        }
        Some(current) => {
            let edited = edit_project_sync_config(&current, fields, enabled, now)?;
            Ok(rotate_project_sync_token(&edited, encrypted_token, now))
        }
    }
}

/// The one entry point both the ticker and the "Sync now" button call: runs
/// [`reconcile_project`] and *always* stamps the outcome via
/// `ProjectSyncConfigRepository::record_sync_result` — success or failure —
/// so neither caller can forget it. A failure recording the result itself is
/// logged and non-fatal (mirrors `crate::use_cases::indexing`'s policy for
/// the same reason: the reconciliation work already happened and must not
/// be reported as lost just because the bookkeeping write failed too).
pub async fn run_and_record(
    ports: &SyncPorts<'_>,
    client: &dyn IssueTrackerClient,
    role: Option<Role>,
    config: &ProjectSyncConfig,
) -> Result<SyncOutcome, AppError> {
    if !is_allowed(role, Action::ManageProjectSync) {
        return Err(AppError::Forbidden);
    }
    let now = ports.clock.now();
    let result = reconcile_project(ports, client, config).await;
    let error_message = result.as_ref().err().map(ToString::to_string);
    if let Err(err) = ports
        .configs
        .record_sync_result(config.project_id, now, error_message.as_deref())
        .await
    {
        eprintln!(
            "anamnesis: failed to record sync result for project {}: {err}",
            config.project_id
        );
    }
    result
}
