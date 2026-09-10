//! The reconciliation algorithm proper: given a project's [`ProjectSyncConfig`]
//! and an [`IssueTrackerClient`], bring its tasks and the linked repo's
//! issues into agreement. [`reconcile_project`] is the whole thing, called
//! only by `super::run_and_record` (never directly, so the outcome is
//! always stamped). Five steps, each a small named sub-function:
//!
//! 1. Push local tasks with no link yet as new issues (only if
//!    `auto_push_new_tasks`), skipping any already sitting in an `is_done`
//!    board column — no point creating an issue only to immediately close
//!    it. See [`tasks_needing_a_new_issue`].
//! 2. Re-read links (step 1 may have inserted rows the next step's own
//!    "never linked?" check needs to see, or a just-pushed issue would be
//!    mistaken for a brand-new *remote* one and imported as a duplicate
//!    task).
//! 3. Pull remote issues updated since the oldest link's watermark, split
//!    into never-linked (imported as new tasks, only if
//!    `auto_import_new_issues`) and already-linked.
//! 4. Reconcile each already-linked task: [`decide_sync_direction`] compares
//!    timestamps and pushes or pulls title, description, and state (open vs.
//!    closed, mapped onto `Task.archived_at`) together, in the one direction
//!    that's due — ordinary two-way sync, no special-casing either way.
//! 5. Import new remote comments on every link, deduped by external comment
//!    id, annotated with their origin.

use std::collections::{HashMap, HashSet};

use anamnesis_core::{self as core, Column, Task, Timestamp};

use crate::entities::{self, CommentId, CommentOrigin};
use crate::error::AppError;
use crate::ports::{
    IssueEdit, IssueState, IssueTrackerClient, RemoteComment, RemoteIssue, TaskAggregate,
    TaskUpdateError,
};
use crate::sync::{ProjectSyncConfig, SyncProvider, TaskSyncLink, link_task_to_issue};
use crate::use_cases::indexing::log_index_failure;

use super::{SyncOutcome, SyncPorts};

pub(super) async fn reconcile_project(
    ports: &SyncPorts<'_>,
    client: &dyn IssueTrackerClient,
    config: &ProjectSyncConfig,
) -> Result<SyncOutcome, AppError> {
    let now = ports.clock.now();
    let (pushed_new_task_count, imported_task_count, known) =
        sync_issue_lists(ports, client, config, now).await?;

    let (pushed_to_remote_count, pulled_from_remote_count) =
        reconcile_known_issues(ports, client, config, known, now).await?;

    let final_links = ports.links.list_for_project(config.project_id).await?;
    let imported_comment_count =
        import_all_new_comments(ports, client, config, &final_links, now).await?;

    Ok(SyncOutcome {
        pushed_new_task_count,
        imported_task_count,
        pushed_to_remote_count,
        pulled_from_remote_count,
        imported_comment_count,
    })
}

/// Steps 1-3 of this module's doc comment: pushes new local tasks as
/// issues, then pulls the remote issue list and splits it into
/// never-linked (imported as tasks, count returned) vs. already-linked
/// (returned for the caller to reconcile). Kept together in one function
/// because step 3's watermark and "never linked?" check must both be
/// computed from links step 1 may have just inserted (see the doc
/// comment's "re-read" note) — splitting them further would just move that
/// coupling to the caller instead of removing it.
async fn sync_issue_lists(
    ports: &SyncPorts<'_>,
    client: &dyn IssueTrackerClient,
    config: &ProjectSyncConfig,
    now: Timestamp,
) -> Result<(usize, usize, Vec<(RemoteIssue, TaskSyncLink)>), AppError> {
    let existing_links = ports.links.list_for_project(config.project_id).await?;
    let pushed_new_task_count = if config.auto_push_new_tasks {
        push_new_local_tasks_as_issues(ports, client, config, &existing_links, now).await?
    } else {
        0
    };

    // Re-read: the step above may have inserted new link rows, and the
    // split below must see them.
    let links_after_push = ports.links.list_for_project(config.project_id).await?;
    let since = oldest_remote_watermark(&links_after_push);
    let remote_issues = client
        .list_issues_since(&config.owner, &config.repo, since)
        .await?;
    let (new_issues, known) = partition_by_link(remote_issues, &links_after_push);

    let imported_task_count = if config.auto_import_new_issues {
        import_new_issues(ports, config, &new_issues, now).await?
    } else {
        0
    };

    Ok((pushed_new_task_count, imported_task_count, known))
}

// --- Step 1: push new local tasks as issues ---

/// Tasks that need a brand-new issue: unlinked, and not already sitting in
/// an `is_done` board column — reuses the exact predicate the archive sweep
/// itself uses (`anamnesis_core::sweep_done`), not a hand-rolled done-check.
fn tasks_needing_a_new_issue(
    tasks: Vec<Task>,
    links: &[TaskSyncLink],
    columns: &[Column],
    now: Timestamp,
) -> Vec<Task> {
    let linked: HashSet<_> = links.iter().map(|l| l.task_id).collect();
    tasks
        .into_iter()
        .filter(|t| !linked.contains(&t.id))
        .filter(|t| core::sweep_done(std::slice::from_ref(t), columns, now).is_empty())
        .collect()
}

async fn push_new_local_tasks_as_issues(
    ports: &SyncPorts<'_>,
    client: &dyn IssueTrackerClient,
    config: &ProjectSyncConfig,
    existing_links: &[TaskSyncLink],
    now: Timestamp,
) -> Result<usize, AppError> {
    let tasks = ports.tasks.list_by_project(config.project_id).await?;
    let board_columns = ports.board.columns_with_items().await?;
    let columns: Vec<Column> = board_columns.into_iter().map(|bc| bc.column).collect();
    let candidates = tasks_needing_a_new_issue(tasks, existing_links, &columns, now);

    let mut count = 0;
    for task in candidates {
        let issue = client
            .create_issue(
                &config.owner,
                &config.repo,
                task.title.as_str(),
                &task.description,
            )
            .await?;
        let link = link_task_to_issue(
            task.id,
            config.project_id,
            issue.number,
            &issue.html_url,
            issue.updated_at,
            now,
        )?;
        ports.links.insert(&link).await?;
        count += 1;
    }
    Ok(count)
}

// --- Step 3: pull new remote issues ---

/// The earliest remote watermark across all links, or `None` with no links
/// at all (first sync — fetch everything). The *minimum*, not the maximum:
/// using the least-recently-confirmed link as the cursor guarantees no
/// update for *any* linked issue is missed, at the cost of occasionally
/// re-fetching an issue that is already current (idempotent, harmless).
fn oldest_remote_watermark(links: &[TaskSyncLink]) -> Option<Timestamp> {
    links.iter().map(|l| l.last_remote_updated_at).min()
}

/// Splits `remote_issues` into "never linked" and "linked, paired with its
/// link row" by `external_issue_number`.
fn partition_by_link(
    remote_issues: Vec<RemoteIssue>,
    links: &[TaskSyncLink],
) -> (Vec<RemoteIssue>, Vec<(RemoteIssue, TaskSyncLink)>) {
    let by_issue: HashMap<u64, TaskSyncLink> = links
        .iter()
        .map(|l| (l.external_issue_number, l.clone()))
        .collect();
    let mut new_issues = Vec::new();
    let mut known = Vec::new();
    for issue in remote_issues {
        match by_issue.get(&issue.number) {
            Some(link) => known.push((issue, link.clone())),
            None => new_issues.push(issue),
        }
    }
    (new_issues, known)
}

async fn import_new_issues(
    ports: &SyncPorts<'_>,
    config: &ProjectSyncConfig,
    issues: &[RemoteIssue],
    now: Timestamp,
) -> Result<usize, AppError> {
    let mut count = 0;
    for issue in issues {
        let task = core::create_task(
            core::TaskId::new(ports.ids.next()),
            config.project_id,
            &issue.title,
            &issue.body,
            now,
        )?;
        ports.tasks.insert(&task).await?;
        if let Err(err) = ports.search.index_task(task.id, task.title.as_str()).await {
            log_index_failure("sync_import_new_issue", err);
        }
        let link = link_task_to_issue(
            task.id,
            config.project_id,
            issue.number,
            &issue.html_url,
            issue.updated_at,
            now,
        )?;
        ports.links.insert(&link).await?;
        count += 1;
    }
    Ok(count)
}

// --- Step 4: reconcile already-linked tasks ---

/// Which way (if any) a linked task's title/description/state need to move,
/// given both sides' timestamps since the last successful sync. Double-edit
/// policy (both sides changed): the later-timestamped side wins, ties favor
/// local — stated here explicitly rather than left as an accident of branch
/// order. True 3-way merge is out of scope.
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

async fn reconcile_known_issues(
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

// --- Step 5: import new comments ---

fn comment_origin(provider: SyncProvider, rc: &RemoteComment) -> CommentOrigin {
    CommentOrigin {
        provider,
        external_comment_id: rc.id,
        external_url: rc.html_url.clone(),
        external_author_display: rc.author_display.clone(),
    }
}

async fn import_all_new_comments(
    ports: &SyncPorts<'_>,
    client: &dyn IssueTrackerClient,
    config: &ProjectSyncConfig,
    links: &[TaskSyncLink],
    now: Timestamp,
) -> Result<usize, AppError> {
    let mut total = 0;
    for link in links {
        total += import_comments_for_link(ports, client, config, link, now).await?;
    }
    Ok(total)
}

async fn import_comments_for_link(
    ports: &SyncPorts<'_>,
    client: &dyn IssueTrackerClient,
    config: &ProjectSyncConfig,
    link: &TaskSyncLink,
    now: Timestamp,
) -> Result<usize, AppError> {
    let remote_comments = client
        .list_comments_since(
            &config.owner,
            &config.repo,
            link.external_issue_number,
            link.last_comment_synced_at,
        )
        .await?;
    let mut latest = link.last_comment_synced_at;
    let mut count = 0;
    for rc in &remote_comments {
        if ports
            .comments
            .exists_with_external_comment_id(link.task_id, rc.id)
            .await?
        {
            continue;
        }
        let origin = comment_origin(config.provider, rc);
        let comment = entities::import_comment(
            CommentId::new(ports.ids.next()),
            link.task_id,
            &rc.body,
            origin,
            now,
        )?;
        ports.comments.insert(&comment).await?;
        latest = Some(latest.map_or(rc.created_at, |l| l.max(rc.created_at)));
        count += 1;
    }
    if count > 0 {
        ports
            .links
            .update(&TaskSyncLink {
                last_comment_synced_at: latest,
                ..link.clone()
            })
            .await?;
    }
    Ok(count)
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

    #[test]
    fn oldest_remote_watermark_is_none_with_no_links() {
        assert_eq!(oldest_remote_watermark(&[]), None);
    }

    #[test]
    fn oldest_remote_watermark_is_the_minimum_across_links() {
        let links = vec![link_at(0, 30), link_at(0, 10), link_at(0, 20)];
        assert_eq!(oldest_remote_watermark(&links), Some(ts(10)));
    }

    fn remote_issue(number: u64) -> RemoteIssue {
        RemoteIssue {
            number,
            title: "Title".to_string(),
            body: "Body".to_string(),
            html_url: format!("https://example.test/issues/{number}"),
            state: IssueState::Open,
            updated_at: ts(0),
        }
    }

    #[test]
    fn partition_by_link_splits_new_from_known_issues() {
        let mut link = link_at(0, 0);
        link.external_issue_number = 5;
        let (new_issues, known) =
            partition_by_link(vec![remote_issue(5), remote_issue(6)], &[link]);
        assert_eq!(new_issues.len(), 1);
        assert_eq!(new_issues[0].number, 6);
        assert_eq!(known.len(), 1);
        assert_eq!(known[0].0.number, 5);
    }

    fn done_column() -> Column {
        anamnesis_core::create_column(
            anamnesis_core::ColumnId::from_u128(1),
            "Done",
            0,
            None,
            true,
        )
        .unwrap()
    }

    fn doing_column() -> Column {
        anamnesis_core::create_column(
            anamnesis_core::ColumnId::from_u128(2),
            "Doing",
            1,
            None,
            false,
        )
        .unwrap()
    }

    fn on_board(task: Task, column: &Column) -> Task {
        core::move_placement(
            &task,
            anamnesis_core::Placement::OnBoard {
                column: column.id,
                position: 0,
            },
            ts(0),
        )
        .unwrap()
    }

    #[test]
    fn tasks_needing_a_new_issue_excludes_linked_and_already_done_tasks() {
        let below = task(false);
        let mut on_done = core::create_task(tid(2), pid(), "Done task", "", ts(0)).unwrap();
        on_done = on_board(on_done, &done_column());
        let mut on_doing = core::create_task(tid(3), pid(), "Doing task", "", ts(0)).unwrap();
        on_doing = on_board(on_doing, &doing_column());
        let mut linked = core::create_task(tid(4), pid(), "Linked task", "", ts(0)).unwrap();
        linked = on_board(linked.clone(), &doing_column());

        let mut link = link_at(0, 0);
        link.task_id = linked.id;

        let candidates = tasks_needing_a_new_issue(
            vec![below.clone(), on_done, on_doing.clone(), linked],
            &[link],
            &[done_column(), doing_column()],
            ts(10),
        );

        let ids: Vec<TaskId> = candidates.iter().map(|t| t.id).collect();
        assert!(ids.contains(&below.id));
        assert!(ids.contains(&on_doing.id));
        assert_eq!(
            ids.len(),
            2,
            "the done task and the linked task must both be excluded"
        );
    }
}
