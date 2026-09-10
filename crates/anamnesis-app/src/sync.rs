//! Project-level sync configuration and per-task issue links (issues #40,
//! #41: "sync with GitHub" / "sync with Forgejo").
//!
//! Lives here, not in `anamnesis-core`, for the same reason `entities.rs`
//! gives for `Comment`/`Attachment`: this is an integration concern with no
//! pure business rule of its own (no invariant like "a project's status
//! can't skip states") — just a flat config record and a link table, each
//! validated minimally in the constructors below.

use serde::{Deserialize, Serialize};

use anamnesis_core::{ProjectId, TaskId, Timestamp};

use crate::error::AppError;

/// Which external issue tracker a [`ProjectSyncConfig`] talks to. Both speak
/// the same Gitea-compatible REST shape (`crate::ports::IssueTrackerClient`)
/// — this enum only selects the concrete adapter and the default host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncProvider {
    GitHub,
    Forgejo,
}

/// The one external repo a project syncs its tasks against — a singleton
/// per project, like [`crate::Settings`] is a singleton globally, hence
/// [`crate::ports::ProjectSyncConfigRepository::upsert`] rather than
/// separate insert/update methods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSyncConfig {
    pub project_id: ProjectId,
    pub provider: SyncProvider,
    /// `None` only for a `GitHub` project pointed at github.com (the HTTP
    /// adapter defaults it to `https://api.github.com`). Required for
    /// GitHub Enterprise and for every Forgejo instance — Forgejo is
    /// self-hosted by definition, so there is no public host to default to.
    pub base_url: Option<String>,
    pub owner: String,
    pub repo: String,
    /// `crate::ports::TokenCipher::encrypt`'s output (nonce || ciphertext).
    /// The plaintext personal access token is never persisted.
    pub encrypted_token: Vec<u8>,
    pub enabled: bool,
    /// Whether a *remote* issue never seen before becomes a new local task.
    pub auto_import_new_issues: bool,
    /// Whether a *local* task with no link yet gets pushed as a new remote
    /// issue. Symmetric with `auto_import_new_issues` by design — the two
    /// toggles are independent.
    pub auto_push_new_tasks: bool,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub last_synced_at: Option<Timestamp>,
    pub last_sync_error: Option<String>,
}

fn non_blank(raw: impl AsRef<str>, what: &str) -> Result<String, AppError> {
    let trimmed = raw.as_ref().trim();
    if trimmed.is_empty() {
        return Err(AppError::Invalid(format!("{what} must not be empty")));
    }
    Ok(trimmed.to_string())
}

/// Validates `base_url` against `provider`: required (and non-blank) for
/// `Forgejo` (self-hosted, no public default host), optional for `GitHub`
/// (defaults to github.com when omitted, but still validated non-blank when
/// given, e.g. for GitHub Enterprise).
fn validate_base_url(
    provider: SyncProvider,
    base_url: Option<String>,
) -> Result<Option<String>, AppError> {
    match (provider, base_url) {
        (SyncProvider::Forgejo, None) => Err(AppError::Invalid(
            "a Forgejo instance URL is required".to_string(),
        )),
        (_, None) => Ok(None),
        (_, Some(raw)) => Ok(Some(non_blank(raw, "the instance URL")?)),
    }
}

/// Builds a new [`ProjectSyncConfig`].
#[allow(clippy::too_many_arguments)]
pub fn configure_project_sync(
    project_id: ProjectId,
    provider: SyncProvider,
    base_url: Option<String>,
    owner: impl AsRef<str>,
    repo: impl AsRef<str>,
    encrypted_token: Vec<u8>,
    auto_import_new_issues: bool,
    auto_push_new_tasks: bool,
    now: Timestamp,
) -> Result<ProjectSyncConfig, AppError> {
    Ok(ProjectSyncConfig {
        project_id,
        provider,
        base_url: validate_base_url(provider, base_url)?,
        owner: non_blank(owner, "the repository owner")?,
        repo: non_blank(repo, "the repository name")?,
        encrypted_token,
        enabled: true,
        auto_import_new_issues,
        auto_push_new_tasks,
        created_at: now,
        updated_at: now,
        last_synced_at: None,
        last_sync_error: None,
    })
}

/// Replaces owner/repo/provider/base-url/the two auto-sync toggles/enabled,
/// stamping `updated_at`. Token rotation is a separate function
/// ([`rotate_project_sync_token`]) because the web form's "leave blank to
/// keep the current token" UX needs to distinguish "no new secret supplied"
/// from "clear the secret", which an `Option<Vec<u8>>` parameter here would
/// read ambiguously at call sites.
#[allow(clippy::too_many_arguments)]
pub fn edit_project_sync_config(
    config: &ProjectSyncConfig,
    provider: SyncProvider,
    base_url: Option<String>,
    owner: impl AsRef<str>,
    repo: impl AsRef<str>,
    auto_import_new_issues: bool,
    auto_push_new_tasks: bool,
    enabled: bool,
    now: Timestamp,
) -> Result<ProjectSyncConfig, AppError> {
    Ok(ProjectSyncConfig {
        provider,
        base_url: validate_base_url(provider, base_url)?,
        owner: non_blank(owner, "the repository owner")?,
        repo: non_blank(repo, "the repository name")?,
        auto_import_new_issues,
        auto_push_new_tasks,
        enabled,
        updated_at: now,
        ..config.clone()
    })
}

/// Replaces the stored ciphertext, stamping `updated_at`.
pub fn rotate_project_sync_token(
    config: &ProjectSyncConfig,
    encrypted_token: Vec<u8>,
    now: Timestamp,
) -> ProjectSyncConfig {
    ProjectSyncConfig {
        encrypted_token,
        updated_at: now,
        ..config.clone()
    }
}

/// One Task <-> external Issue link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSyncLink {
    pub task_id: TaskId,
    pub project_id: ProjectId,
    /// Per-repo issue number (GitHub/Forgejo issue numbers are not globally
    /// unique) — what `/repos/{owner}/{repo}/issues/{number}` needs.
    pub external_issue_number: u64,
    pub external_url: String,
    /// The remote issue's own `updated_at` as of the last successful sync in
    /// either direction.
    pub last_remote_updated_at: Timestamp,
    /// The local task's `last_touched_at` as of the last successful sync in
    /// either direction.
    pub last_local_synced_at: Timestamp,
    /// Watermark for comment import; `None` before the first comment sync.
    pub last_comment_synced_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

/// Links a task to an external issue, seeding both watermarks to `now` (or,
/// for the remote watermark, to the issue's own `remote_updated_at` when
/// this is created from a pulled remote issue rather than a freshly pushed
/// one).
pub fn link_task_to_issue(
    task_id: TaskId,
    project_id: ProjectId,
    external_issue_number: u64,
    external_url: impl AsRef<str>,
    remote_updated_at: Timestamp,
    now: Timestamp,
) -> Result<TaskSyncLink, AppError> {
    Ok(TaskSyncLink {
        task_id,
        project_id,
        external_issue_number,
        external_url: non_blank(external_url, "the issue URL")?,
        last_remote_updated_at: remote_updated_at,
        last_local_synced_at: now,
        last_comment_synced_at: None,
        created_at: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use uuid::Uuid;

    fn pid() -> ProjectId {
        ProjectId::new(Uuid::from_u128(1))
    }

    fn tid() -> TaskId {
        TaskId::new(Uuid::from_u128(2))
    }

    fn ts(secs: i64) -> Timestamp {
        Timestamp::from_unix_seconds(secs).unwrap()
    }

    #[test]
    fn configure_project_sync_builds_a_github_config_with_no_base_url() {
        let config = configure_project_sync(
            pid(),
            SyncProvider::GitHub,
            None,
            "octocat",
            "hello-world",
            vec![1, 2, 3],
            true,
            true,
            ts(0),
        )
        .unwrap();
        assert_eq!(config.base_url, None);
        assert!(config.enabled);
        assert_eq!(config.last_synced_at, None);
    }

    #[test]
    fn configure_project_sync_rejects_forgejo_without_a_base_url() {
        let result = configure_project_sync(
            pid(),
            SyncProvider::Forgejo,
            None,
            "octocat",
            "hello-world",
            vec![],
            true,
            true,
            ts(0),
        );
        assert!(matches!(result, Err(AppError::Invalid(_))));
    }

    #[test]
    fn configure_project_sync_rejects_a_blank_forgejo_base_url() {
        let result = configure_project_sync(
            pid(),
            SyncProvider::Forgejo,
            Some("   ".to_string()),
            "octocat",
            "hello-world",
            vec![],
            true,
            true,
            ts(0),
        );
        assert!(matches!(result, Err(AppError::Invalid(_))));
    }

    #[test]
    fn configure_project_sync_accepts_a_forgejo_base_url() {
        let config = configure_project_sync(
            pid(),
            SyncProvider::Forgejo,
            Some("https://forgejo.example.com".to_string()),
            "octocat",
            "hello-world",
            vec![],
            true,
            true,
            ts(0),
        )
        .unwrap();
        assert_eq!(config.base_url.as_deref(), Some("https://forgejo.example.com"));
    }

    #[rstest]
    #[case("", "hello-world")]
    #[case("octocat", "")]
    #[case("   ", "hello-world")]
    fn configure_project_sync_rejects_blank_owner_or_repo(#[case] owner: &str, #[case] repo: &str) {
        let result = configure_project_sync(
            pid(),
            SyncProvider::GitHub,
            None,
            owner,
            repo,
            vec![],
            true,
            true,
            ts(0),
        );
        assert!(matches!(result, Err(AppError::Invalid(_))));
    }

    #[test]
    fn edit_project_sync_config_replaces_fields_and_stamps_updated_at() {
        let config = configure_project_sync(
            pid(),
            SyncProvider::GitHub,
            None,
            "octocat",
            "hello-world",
            vec![],
            true,
            true,
            ts(0),
        )
        .unwrap();
        let edited = edit_project_sync_config(
            &config,
            SyncProvider::GitHub,
            None,
            "octocat",
            "renamed-repo",
            false,
            false,
            false,
            ts(10),
        )
        .unwrap();
        assert_eq!(edited.repo, "renamed-repo");
        assert!(!edited.auto_import_new_issues);
        assert!(!edited.auto_push_new_tasks);
        assert!(!edited.enabled);
        assert_eq!(edited.updated_at, ts(10));
        assert_eq!(edited.encrypted_token, config.encrypted_token);
    }

    #[test]
    fn rotate_project_sync_token_replaces_only_the_token() {
        let config = configure_project_sync(
            pid(),
            SyncProvider::GitHub,
            None,
            "octocat",
            "hello-world",
            vec![1],
            true,
            true,
            ts(0),
        )
        .unwrap();
        let rotated = rotate_project_sync_token(&config, vec![9, 9, 9], ts(5));
        assert_eq!(rotated.encrypted_token, vec![9, 9, 9]);
        assert_eq!(rotated.updated_at, ts(5));
        assert_eq!(rotated.owner, config.owner);
    }

    #[test]
    fn link_task_to_issue_seeds_both_watermarks() {
        let link = link_task_to_issue(
            tid(),
            pid(),
            42,
            "https://github.com/octocat/hello-world/issues/42",
            ts(3),
            ts(10),
        )
        .unwrap();
        assert_eq!(link.external_issue_number, 42);
        assert_eq!(link.last_remote_updated_at, ts(3));
        assert_eq!(link.last_local_synced_at, ts(10));
        assert_eq!(link.last_comment_synced_at, None);
    }

    #[test]
    fn link_task_to_issue_rejects_a_blank_url() {
        let result = link_task_to_issue(tid(), pid(), 1, "  ", ts(0), ts(0));
        assert!(matches!(result, Err(AppError::Invalid(_))));
    }
}
