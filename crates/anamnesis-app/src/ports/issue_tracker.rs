//! [`IssueTrackerClient`]: one HTTP port shared by GitHub and Forgejo (issues
//! #40, #41) — Forgejo's REST API is Gitea-compatible and exposes the same
//! `/repos/{owner}/{repo}/issues...` shape GitHub does, so both providers
//! implement this one trait rather than getting a trait each.

use async_trait::async_trait;

use anamnesis_core::Timestamp;

use crate::error::IssueTrackerError;

/// The only remote lifecycle state either API exposes — both GitHub's and
/// Forgejo's issue JSON carries exactly this as `"state": "open"|"closed"`.
/// anamnesis's finer board states (Doing, Todo, ...) have no remote
/// equivalent and are never synced; only this one binary distinction maps
/// onto `Task.archived_at.is_some()`, ordinarily and two-way, exactly like
/// title/description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueState {
    Open,
    Closed,
}

/// One issue as reported by the tracker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteIssue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub html_url: String,
    pub state: IssueState,
    pub updated_at: Timestamp,
}

/// One comment on an issue, as reported by the tracker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteComment {
    pub id: u64,
    pub body: String,
    pub html_url: String,
    /// The external system's display name for whoever posted it (a GitHub
    /// login, a Forgejo username) — not an anamnesis `UserId`.
    pub author_display: String,
    pub created_at: Timestamp,
}

/// What [`IssueTrackerClient::update_issue`] changes. `title`/`body`/
/// `state` always travel together — reconciliation makes one direction
/// decision per linked task per tick, not one per field.
#[derive(Debug, Clone, Copy)]
pub struct IssueEdit<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub state: IssueState,
}

/// One external issue tracker repo: GitHub or a self-hosted Forgejo
/// instance, both implemented by the same concrete adapter
/// (`anamnesis_adapters::HttpIssueTrackerClient`) since their REST shapes
/// are identical.
#[async_trait]
pub trait IssueTrackerClient: Send + Sync {
    /// Every issue in `owner/repo` updated at or after `since` (`None`
    /// means every open and closed issue — a project's first sync). The
    /// adapter follows pagination itself and filters out pull requests
    /// (both APIs mix them into the issues list, distinguishable by a
    /// `pull_request` key), so callers always see a clean issue set.
    async fn list_issues_since(
        &self,
        owner: &str,
        repo: &str,
        since: Option<Timestamp>,
    ) -> Result<Vec<RemoteIssue>, IssueTrackerError>;
    /// Creates a new issue, always open.
    async fn create_issue(
        &self,
        owner: &str,
        repo: &str,
        title: &str,
        body: &str,
    ) -> Result<RemoteIssue, IssueTrackerError>;
    /// Returns the issue as the server now has it — both GitHub's and
    /// Forgejo's PATCH response bodies already include the updated resource,
    /// so this costs no extra request. Callers need the server's own
    /// `updated_at` back (not their own clock's "now") for their next
    /// watermark: assuming a local clock and the server's clock agree
    /// closely enough to substitute one for the other would risk a
    /// just-pushed change going missing from a future `list_issues_since`
    /// poll the moment they disagree even slightly.
    async fn update_issue(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        edit: IssueEdit<'_>,
    ) -> Result<RemoteIssue, IssueTrackerError>;
    /// Every comment on issue `number` created at or after `since` (`None`
    /// means every comment — a link's first comment sync).
    async fn list_comments_since(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        since: Option<Timestamp>,
    ) -> Result<Vec<RemoteComment>, IssueTrackerError>;
}
