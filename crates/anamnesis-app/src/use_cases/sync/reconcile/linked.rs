//! Step 4 of `super`'s doc comment: reconciling tasks that already have a
//! link. Which way (if any) a linked task's title/description/state need to
//! move, given both sides' timestamps since the last successful sync.
//! Double-edit policy (both sides changed): the later-timestamped side
//! wins, ties favor local — stated here explicitly rather than left as an
//! accident of branch order. True 3-way merge is out of scope.

use anamnesis_core::{self as core, Task, Timestamp};

use crate::error::AppError;
use crate::ports::{
    IssueEdit, IssueState, IssueTrackerClient, RemoteIssue, TaskAggregate, TaskUpdateError,
};
use crate::sync::{ProjectSyncConfig, TaskSyncLink};
use crate::use_cases::indexing::log_index_failure;

use crate::use_cases::sync::SyncPorts;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncDirection {
    NoChange,
    PushLocalToRemote,
    PullRemoteToLocal,
}

fn decide_sync_direction(
    link: &TaskSyncLink,
    task_last_touched_at: Timestamp,
    issue_updated_at: Timestamp,
) -> SyncDirection {
    let local_changed = task_last_touched_at > link.last_local_synced_at;
    let remote_changed = issue_updated_at > link.last_remote_updated_at;
    match (local_changed, remote_changed) {
        (false, false) => SyncDirection::NoChange,
        (true, false) => SyncDirection::PushLocalToRemote,
        (false, true) => SyncDirection::PullRemoteToLocal,
        (true, true) if task_last_touched_at >= issue_updated_at => {
            SyncDirection::PushLocalToRemote
        }
        (true, true) => SyncDirection::PullRemoteToLocal,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkedTaskOutcome {
    NoChange,
    Pushed,
    Pulled,
    SkippedConflict,
    SkippedTaskMissing,
}

pub(super) async fn reconcile_known_issues(
    ports: &SyncPorts<'_>,
    client: &dyn IssueTrackerClient,
    config: &ProjectSyncConfig,
    known: Vec<(RemoteIssue, TaskSyncLink)>,
    now: Timestamp,
) -> Result<(usize, usize), AppError> {
    let mut pushed = 0usize;
    let mut pulled = 0usize;
    for (issue, link) in known {
        match reconcile_linked_task(ports, client, config, &issue, &link, now).await? {
            LinkedTaskOutcome::Pushed => pushed += 1,
            LinkedTaskOutcome::Pulled => pulled += 1,
            LinkedTaskOutcome::NoChange
            | LinkedTaskOutcome::SkippedConflict
            | LinkedTaskOutcome::SkippedTaskMissing => {}
        }
    }
    Ok((pushed, pulled))
}

async fn reconcile_linked_task(
    ports: &SyncPorts<'_>,
    client: &dyn IssueTrackerClient,
    config: &ProjectSyncConfig,
    issue: &RemoteIssue,
    link: &TaskSyncLink,
    now: Timestamp,
) -> Result<LinkedTaskOutcome, AppError> {
    // A stale link from a task deleted out from under it shouldn't fail the
    // whole project's pass.
    let Some(aggregate) = ports.tasks.load(link.task_id).await? else {
        return Ok(LinkedTaskOutcome::SkippedTaskMissing);
    };
    match decide_sync_direction(link, aggregate.task.last_touched_at, issue.updated_at) {
        SyncDirection::NoChange => Ok(LinkedTaskOutcome::NoChange),
        SyncDirection::PushLocalToRemote => {
            apply_push(ports, client, config, &aggregate.task, link, now).await
        }
        SyncDirection::PullRemoteToLocal => apply_pull(ports, &aggregate, issue, link, now).await,
    }
}

fn task_issue_state(task: &Task) -> IssueState {
    if task.archived_at.is_some() {
        IssueState::Closed
    } else {
        IssueState::Open
    }
}

async fn apply_push(
    ports: &SyncPorts<'_>,
    client: &dyn IssueTrackerClient,
    config: &ProjectSyncConfig,
    task: &Task,
    link: &TaskSyncLink,
    now: Timestamp,
) -> Result<LinkedTaskOutcome, AppError> {
    let updated_issue = client
        .update_issue(
            &config.owner,
            &config.repo,
            link.external_issue_number,
            IssueEdit {
                title: task.title.as_str(),
                body: &task.description,
                state: task_issue_state(task),
            },
        )
        .await?;
    // The remote watermark is the server's own `updated_at` from the PATCH
    // response, not our own clock's `now` — substituting one clock for the
    // other would risk this very push going missing from a future
    // `list_issues_since` poll the moment the two disagree even slightly.
    ports
        .links
        .update(&TaskSyncLink {
            last_remote_updated_at: updated_issue.updated_at,
            last_local_synced_at: now,
            ..link.clone()
        })
        .await?;
    Ok(LinkedTaskOutcome::Pushed)
}

/// Applies the remote's open/closed state on top of an already-edited task —
/// the mirror of [`task_issue_state`]. Archiving/unarchiving is ordinary,
/// two-way sync here, exactly like title/description: closing (or
/// reopening) the issue upstream archives (or unarchives) the linked task.
fn apply_remote_state(task: &Task, state: IssueState, now: Timestamp) -> Result<Task, AppError> {
    match (state, task.archived_at.is_some()) {
        (IssueState::Closed, false) => Ok(core::archive_task(task, now)?),
        (IssueState::Open, true) => Ok(core::unarchive_task(task, now)?),
        _ => Ok(task.clone()),
    }
}

/// Keeps the search index in step with an archived-state transition applied
/// by [`apply_remote_state`] — mirrors what
/// `crate::use_cases::task::archive_task`/`unarchive_task` do for the same
/// transition reached through the ordinary UI path.
async fn note_archival_change(ports: &SyncPorts<'_>, before: &Task, after: &Task) {
    if before.archived_at.is_none()
        && after.archived_at.is_some()
        && let Err(err) = ports.search.remove_task(after.id).await
    {
        log_index_failure("sync_pull_archive", err);
    } else if before.archived_at.is_some()
        && after.archived_at.is_none()
        && let Err(err) = ports
            .search
            .index_task(after.id, after.title.as_str())
            .await
    {
        log_index_failure("sync_pull_unarchive", err);
    }
}

async fn apply_pull(
    ports: &SyncPorts<'_>,
    aggregate: &TaskAggregate,
    issue: &RemoteIssue,
    link: &TaskSyncLink,
    now: Timestamp,
) -> Result<LinkedTaskOutcome, AppError> {
    let edited = core::edit_task(&aggregate.task, &issue.title, &issue.body, now)?;
    let final_task = apply_remote_state(&edited, issue.state, now)?;
    match ports
        .tasks
        .update(&final_task, aggregate.task.last_touched_at)
        .await
    {
        Ok(()) => {
            note_archival_change(ports, &aggregate.task, &final_task).await;
            ports
                .links
                .update(&TaskSyncLink {
                    last_remote_updated_at: issue.updated_at,
                    last_local_synced_at: now,
                    ..link.clone()
                })
                .await?;
            Ok(LinkedTaskOutcome::Pulled)
        }
        // A real concurrent local edit landed between our load and write.
        // Leave the link untouched — the next tick reconsiders from fresh
        // state — rather than fail the whole project's pass.
        Err(TaskUpdateError::Conflict) => Ok(LinkedTaskOutcome::SkippedConflict),
        Err(TaskUpdateError::Repo(e)) => Err(AppError::Repo(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anamnesis_core::{ProjectId, TaskId};
    use rstest::rstest;
    use uuid::Uuid;

    fn ts(secs: i64) -> Timestamp {
        Timestamp::from_unix_seconds(secs).unwrap()
    }

    fn tid(n: u128) -> TaskId {
        TaskId::new(Uuid::from_u128(n))
    }

    fn pid() -> ProjectId {
        ProjectId::new(Uuid::from_u128(100))
    }

    fn link_at(local_synced: i64, remote_synced: i64) -> TaskSyncLink {
        TaskSyncLink {
            task_id: tid(1),
            project_id: pid(),
            external_issue_number: 1,
            external_url: "https://example.test/issues/1".to_string(),
            last_remote_updated_at: ts(remote_synced),
            last_local_synced_at: ts(local_synced),
            last_comment_synced_at: None,
            created_at: ts(0),
        }
    }

    #[rstest]
    #[case(10, 10, 10, 10, SyncDirection::NoChange)]
    #[case(15, 10, 10, 10, SyncDirection::PushLocalToRemote)]
    #[case(10, 10, 10, 15, SyncDirection::PullRemoteToLocal)]
    #[case(20, 10, 10, 15, SyncDirection::PushLocalToRemote)] // double-edit, local later
    #[case(15, 10, 10, 20, SyncDirection::PullRemoteToLocal)] // double-edit, remote later
    #[case(15, 10, 10, 15, SyncDirection::PushLocalToRemote)] // double-edit tie: local wins
    fn decide_sync_direction_matches_the_stated_policy(
        #[case] task_last_touched_at: i64,
        #[case] last_local_synced_at: i64,
        #[case] last_remote_updated_at: i64,
        #[case] issue_updated_at: i64,
        #[case] expected: SyncDirection,
    ) {
        let link = link_at(last_local_synced_at, last_remote_updated_at);
        let direction =
            decide_sync_direction(&link, ts(task_last_touched_at), ts(issue_updated_at));
        assert_eq!(direction, expected);
    }

    fn task(archived: bool) -> Task {
        let mut t = core::create_task(tid(1), pid(), "Title", "Body", ts(0)).unwrap();
        if archived {
            t = core::archive_task(&t, ts(1)).unwrap();
        }
        t
    }

    #[test]
    fn task_issue_state_maps_archived_to_closed() {
        assert_eq!(task_issue_state(&task(true)), IssueState::Closed);
        assert_eq!(task_issue_state(&task(false)), IssueState::Open);
    }

    #[test]
    fn apply_remote_state_closes_an_open_task_when_the_issue_is_closed() {
        let result = apply_remote_state(&task(false), IssueState::Closed, ts(5)).unwrap();
        assert!(result.archived_at.is_some());
    }

    #[test]
    fn apply_remote_state_reopens_an_archived_task_when_the_issue_is_open() {
        let result = apply_remote_state(&task(true), IssueState::Open, ts(5)).unwrap();
        assert!(result.archived_at.is_none());
    }

    #[rstest]
    #[case(false, IssueState::Open)]
    #[case(true, IssueState::Closed)]
    fn apply_remote_state_is_a_no_op_when_both_sides_already_agree(
        #[case] archived: bool,
        #[case] state: IssueState,
    ) {
        let original = task(archived);
        let result = apply_remote_state(&original, state, ts(5)).unwrap();
        assert_eq!(result, original);
    }
}
