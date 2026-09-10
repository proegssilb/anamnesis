//! Steps 1-3 of `super`'s doc comment: pushes new local tasks as issues,
//! then pulls the remote issue list and splits it into never-linked
//! (imported as tasks, count returned) vs. already-linked (returned for
//! `linked` to reconcile).

use std::collections::{HashMap, HashSet};

use anamnesis_core::{self as core, Column, Task, Timestamp};

use crate::error::AppError;
use crate::ports::{IssueTrackerClient, RemoteIssue};
use crate::sync::{ProjectSyncConfig, TaskSyncLink, link_task_to_issue};

use crate::use_cases::sync::SyncPorts;

/// Kept as one function because the watermark and "never linked?" check
/// below must both be computed from links the push step may have just
/// inserted (a just-pushed issue would otherwise be mistaken for a
/// brand-new *remote* one and imported as a duplicate task) — splitting
/// this further would just move that coupling to the caller instead of
/// removing it.
pub(super) async fn sync_issue_lists(
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
            crate::use_cases::indexing::log_index_failure("sync_import_new_issue", err);
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

#[cfg(test)]
mod tests {
    use super::*;
    use anamnesis_core::{ProjectId, TaskId};
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
            state: crate::ports::IssueState::Open,
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
        let below = core::create_task(tid(1), pid(), "Title", "Body", ts(0)).unwrap();
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
