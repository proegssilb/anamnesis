//! The reconciliation algorithm proper: given a project's [`ProjectSyncConfig`]
//! and an [`IssueTrackerClient`], bring its tasks and the linked repo's
//! issues into agreement. [`reconcile_project`] is the whole thing, called
//! only by `super::run_and_record` (never directly, so the outcome is
//! always stamped). Five steps, one submodule per group of them (matching
//! this doc comment's own numbering, not an arbitrary line-count split):
//!
//! 1-3. [`discover`]: push local tasks with no link yet as new issues (only
//!    if `auto_push_new_tasks`, skipping any already sitting in an
//!    `is_done` board column), then pull remote issues updated since the
//!    oldest link's watermark and split them into never-linked (imported as
//!    new tasks, only if `auto_import_new_issues`) and already-linked.
//! 4. [`linked`]: reconcile each already-linked task — compare timestamps
//!    and push or pull title, description, and state (open vs. closed,
//!    mapped onto `Task.archived_at`) together, in the one direction that's
//!    due — ordinary two-way sync, no special-casing either way.
//! 5. [`comments`]: import new remote comments on every link, deduped by
//!    external comment id, annotated with their origin.

mod comments;
mod discover;
mod linked;

use crate::error::AppError;
use crate::ports::IssueTrackerClient;
use crate::sync::ProjectSyncConfig;

use super::{SyncOutcome, SyncPorts};

pub(super) async fn reconcile_project(
    ports: &SyncPorts<'_>,
    client: &dyn IssueTrackerClient,
    config: &ProjectSyncConfig,
) -> Result<SyncOutcome, AppError> {
    let now = ports.clock.now();
    let (pushed_new_task_count, imported_task_count, known) =
        discover::sync_issue_lists(ports, client, config, now).await?;

    let (pushed_to_remote_count, pulled_from_remote_count) =
        linked::reconcile_known_issues(ports, client, config, known, now).await?;

    let final_links = ports.links.list_for_project(config.project_id).await?;
    let imported_comment_count =
        comments::import_all_new_comments(ports, client, config, &final_links, now).await?;

    Ok(SyncOutcome {
        pushed_new_task_count,
        imported_task_count,
        pushed_to_remote_count,
        pulled_from_remote_count,
        imported_comment_count,
    })
}
