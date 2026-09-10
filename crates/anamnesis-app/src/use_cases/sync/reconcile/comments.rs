//! Step 5 of `super`'s doc comment: importing new remote comments on every
//! link, deduped by external comment id, annotated with their origin.

use anamnesis_core::Timestamp;

use crate::entities::{self, CommentId, CommentOrigin};
use crate::error::AppError;
use crate::ports::{IssueTrackerClient, RemoteComment};
use crate::sync::{ProjectSyncConfig, SyncProvider, TaskSyncLink};

use crate::use_cases::sync::SyncPorts;

fn comment_origin(provider: SyncProvider, rc: &RemoteComment) -> CommentOrigin {
    CommentOrigin {
        provider,
        external_comment_id: rc.id,
        external_url: rc.html_url.clone(),
        external_author_display: rc.author_display.clone(),
    }
}

pub(super) async fn import_all_new_comments(
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
