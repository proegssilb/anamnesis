//! [`HttpIssueTrackerClient`]: [`IssueTrackerClient`] over `reqwest`, shared
//! by GitHub and Forgejo (issues #40/#41) — Forgejo's REST API is
//! Gitea-compatible and exposes the same `/repos/{owner}/{repo}/issues...`
//! shape GitHub does, differing only in API root and auth header form.

use anamnesis_app::{
    IssueEdit, IssueState, IssueTrackerClient, IssueTrackerError, RemoteComment, RemoteIssue,
    SyncProvider,
};
use anamnesis_core::Timestamp;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Requests more than this many results per page from either API.
const PER_PAGE: u32 = 100;

/// How `HttpIssueTrackerClient` sends its token: GitHub accepts (and
/// recommends) `Bearer`; Forgejo/Gitea's own docs use `token`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthScheme {
    Bearer,
    Token,
}

/// One external issue tracker repo. Built via [`Self::github`]/
/// [`Self::forgejo`], or [`build_client`] when the provider is only known at
/// runtime (as it is when loading a [`anamnesis_app::ProjectSyncConfig`]).
pub struct HttpIssueTrackerClient {
    client: reqwest::Client,
    api_root: String,
    token: String,
    auth_scheme: AuthScheme,
}

impl HttpIssueTrackerClient {
    /// A client for github.com, or — when `base_url` names a GitHub
    /// Enterprise host — that instance instead (its API root is
    /// `{base_url}/api/v3`, not the bare host).
    pub fn github(base_url: Option<&str>, token: &str) -> Result<Self, IssueTrackerError> {
        let api_root = match base_url {
            Some(host) => format!("{}/api/v3", host.trim_end_matches('/')),
            None => "https://api.github.com".to_string(),
        };
        Self::new(api_root, token, AuthScheme::Bearer)
    }

    /// A client for a self-hosted Forgejo instance at `base_url` (its API
    /// root is `{base_url}/api/v1`).
    pub fn forgejo(base_url: &str, token: &str) -> Result<Self, IssueTrackerError> {
        let api_root = format!("{}/api/v1", base_url.trim_end_matches('/'));
        Self::new(api_root, token, AuthScheme::Token)
    }

    fn new(
        api_root: String,
        token: &str,
        auth_scheme: AuthScheme,
    ) -> Result<Self, IssueTrackerError> {
        let client = reqwest::Client::builder()
            // Neither API issues redirects on these endpoints; following one
            // anyway would risk leaking the Authorization header to a host
            // this config never named.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| IssueTrackerError::from_source("failed to build HTTP client", e))?;
        Ok(Self {
            client,
            api_root,
            token: token.to_string(),
            auth_scheme,
        })
    }

    fn authorize(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let value = match self.auth_scheme {
            AuthScheme::Bearer => format!("Bearer {}", self.token),
            AuthScheme::Token => format!("token {}", self.token),
        };
        builder.header(reqwest::header::AUTHORIZATION, value)
    }
}

/// Builds a client for whichever provider a [`anamnesis_app::ProjectSyncConfig`]
/// names, at runtime. `base_url` is required for `Forgejo` (self-hosted, no
/// public default) — the caller (the reconciliation entry point) is
/// expected to have already validated that via
/// `anamnesis_app::configure_project_sync`, so this treats a missing one as
/// a genuine error rather than re-deriving that validation.
pub fn build_client(
    provider: SyncProvider,
    base_url: Option<&str>,
    token: &str,
) -> Result<HttpIssueTrackerClient, IssueTrackerError> {
    match provider {
        SyncProvider::GitHub => HttpIssueTrackerClient::github(base_url, token),
        SyncProvider::Forgejo => {
            let base_url = base_url
                .ok_or_else(|| IssueTrackerError::new("a Forgejo config is missing its base URL"))?;
            HttpIssueTrackerClient::forgejo(base_url, token)
        }
    }
}

fn issue_state_to_text(state: IssueState) -> &'static str {
    match state {
        IssueState::Open => "open",
        IssueState::Closed => "closed",
    }
}

fn issue_state_from_text(raw: &str) -> Result<IssueState, IssueTrackerError> {
    match raw {
        "open" => Ok(IssueState::Open),
        "closed" => Ok(IssueState::Closed),
        other => Err(IssueTrackerError::new(format!(
            "unrecognized issue state {other:?}"
        ))),
    }
}

fn parse_rfc3339(raw: &str) -> Result<Timestamp, IssueTrackerError> {
    let parsed = time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339)
        .map_err(|e| IssueTrackerError::from_source(format!("invalid timestamp {raw:?}"), e))?;
    Timestamp::from_unix_seconds(parsed.unix_timestamp())
        .map_err(|e| IssueTrackerError::from_source(format!("timestamp out of range: {raw:?}"), e))
}

fn format_rfc3339(ts: Timestamp) -> Result<String, IssueTrackerError> {
    let dt = time::OffsetDateTime::from_unix_timestamp(ts.unix_seconds())
        .map_err(|e| IssueTrackerError::from_source("invalid timestamp", e))?;
    dt.format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| IssueTrackerError::from_source("failed to format timestamp", e))
}

/// One issue as either API's JSON shape has it. `pull_request` is present
/// (non-null) only on a pull request — both APIs mix PRs into the issues
/// list, so this is how callers tell them apart.
#[derive(Debug, Deserialize)]
struct RawIssue {
    number: u64,
    title: String,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    state: String,
    updated_at: String,
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RawUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct RawComment {
    id: u64,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    user: RawUser,
    created_at: String,
}

fn remote_issue_from_raw(raw: RawIssue) -> Result<RemoteIssue, IssueTrackerError> {
    Ok(RemoteIssue {
        number: raw.number,
        title: raw.title,
        body: raw.body.unwrap_or_default(),
        html_url: raw.html_url,
        state: issue_state_from_text(&raw.state)?,
        updated_at: parse_rfc3339(&raw.updated_at)?,
    })
}

fn remote_comment_from_raw(raw: RawComment) -> Result<RemoteComment, IssueTrackerError> {
    Ok(RemoteComment {
        id: raw.id,
        body: raw.body.unwrap_or_default(),
        html_url: raw.html_url,
        author_display: raw.user.login,
        created_at: parse_rfc3339(&raw.created_at)?,
    })
}

#[derive(Debug, Serialize)]
struct IssueCreateBody<'a> {
    title: &'a str,
    body: &'a str,
}

#[derive(Debug, Serialize)]
struct IssueUpdateBody<'a> {
    title: &'a str,
    body: &'a str,
    state: &'a str,
}

/// Turns a non-2xx response into an [`IssueTrackerError`] carrying the
/// status and body — this is also how a 403/429 rate-limit response
/// surfaces: as an ordinary error that aborts the rest of the calling
/// project's reconciliation pass for this tick (see
/// `anamnesis_app::use_cases::sync`'s module doc comment), rather than
/// something this adapter retries on its own.
async fn error_for_status(
    response: reqwest::Response,
    what: &str,
) -> Result<reqwest::Response, IssueTrackerError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(IssueTrackerError::new(format!(
        "failed to {what}: HTTP {status} {body}"
    )))
}

#[async_trait]
impl IssueTrackerClient for HttpIssueTrackerClient {
    async fn list_issues_since(
        &self,
        owner: &str,
        repo: &str,
        since: Option<Timestamp>,
    ) -> Result<Vec<RemoteIssue>, IssueTrackerError> {
        let since_param = since.map(format_rfc3339).transpose()?;
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let url = format!("{}/repos/{owner}/{repo}/issues", self.api_root);
            let page_str = page.to_string();
            let mut params = vec![
                ("state", "all"),
                ("per_page", "100"),
                ("page", page_str.as_str()),
            ];
            if let Some(since) = since_param.as_deref() {
                params.push(("since", since));
            }
            let request = self.authorize(self.client.get(&url).query(&params));
            let response = request
                .send()
                .await
                .map_err(|e| IssueTrackerError::from_source("failed to list issues", e))?;
            let response = error_for_status(response, "list issues").await?;
            let raw_issues: Vec<RawIssue> = response
                .json()
                .await
                .map_err(|e| IssueTrackerError::from_source("failed to parse issues response", e))?;
            let count = raw_issues.len();
            for raw in raw_issues {
                if raw.pull_request.is_some() {
                    continue;
                }
                all.push(remote_issue_from_raw(raw)?);
            }
            if u32::try_from(count).unwrap_or(0) < PER_PAGE {
                break;
            }
            page += 1;
        }
        Ok(all)
    }

    async fn create_issue(
        &self,
        owner: &str,
        repo: &str,
        title: &str,
        body: &str,
    ) -> Result<RemoteIssue, IssueTrackerError> {
        let url = format!("{}/repos/{owner}/{repo}/issues", self.api_root);
        let request = self
            .authorize(self.client.post(&url))
            .json(&IssueCreateBody { title, body });
        let response = request
            .send()
            .await
            .map_err(|e| IssueTrackerError::from_source("failed to create issue", e))?;
        let response = error_for_status(response, "create issue").await?;
        let raw: RawIssue = response
            .json()
            .await
            .map_err(|e| IssueTrackerError::from_source("failed to parse created issue", e))?;
        remote_issue_from_raw(raw)
    }

    async fn update_issue(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        edit: IssueEdit<'_>,
    ) -> Result<RemoteIssue, IssueTrackerError> {
        let url = format!("{}/repos/{owner}/{repo}/issues/{number}", self.api_root);
        let body = IssueUpdateBody {
            title: edit.title,
            body: edit.body,
            state: issue_state_to_text(edit.state),
        };
        let request = self.authorize(self.client.patch(&url)).json(&body);
        let response = request
            .send()
            .await
            .map_err(|e| IssueTrackerError::from_source("failed to update issue", e))?;
        let response = error_for_status(response, "update issue").await?;
        let raw: RawIssue = response
            .json()
            .await
            .map_err(|e| IssueTrackerError::from_source("failed to parse updated issue", e))?;
        remote_issue_from_raw(raw)
    }

    async fn list_comments_since(
        &self,
        owner: &str,
        repo: &str,
        number: u64,
        since: Option<Timestamp>,
    ) -> Result<Vec<RemoteComment>, IssueTrackerError> {
        let since_param = since.map(format_rfc3339).transpose()?;
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let url = format!(
                "{}/repos/{owner}/{repo}/issues/{number}/comments",
                self.api_root
            );
            let page_str = page.to_string();
            let mut params = vec![("per_page", "100"), ("page", page_str.as_str())];
            if let Some(since) = since_param.as_deref() {
                params.push(("since", since));
            }
            let request = self.authorize(self.client.get(&url).query(&params));
            let response = request
                .send()
                .await
                .map_err(|e| IssueTrackerError::from_source("failed to list comments", e))?;
            let response = error_for_status(response, "list comments").await?;
            let raw_comments: Vec<RawComment> = response.json().await.map_err(|e| {
                IssueTrackerError::from_source("failed to parse comments response", e)
            })?;
            let count = raw_comments.len();
            for raw in raw_comments {
                all.push(remote_comment_from_raw(raw)?);
            }
            if u32::try_from(count).unwrap_or(0) < PER_PAGE {
                break;
            }
            page += 1;
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_without_a_base_url_targets_the_public_api() {
        let client = HttpIssueTrackerClient::github(None, "t").unwrap();
        assert_eq!(client.api_root, "https://api.github.com");
        assert_eq!(client.auth_scheme, AuthScheme::Bearer);
    }

    #[test]
    fn github_with_a_base_url_targets_the_enterprise_api_root() {
        let client = HttpIssueTrackerClient::github(Some("https://github.example.com"), "t").unwrap();
        assert_eq!(client.api_root, "https://github.example.com/api/v3");
    }

    #[test]
    fn forgejo_targets_its_own_api_root() {
        let client = HttpIssueTrackerClient::forgejo("https://forgejo.example.com/", "t").unwrap();
        assert_eq!(client.api_root, "https://forgejo.example.com/api/v1");
        assert_eq!(client.auth_scheme, AuthScheme::Token);
    }

    #[test]
    fn build_client_requires_a_base_url_for_forgejo() {
        assert!(build_client(SyncProvider::Forgejo, None, "t").is_err());
        assert!(build_client(SyncProvider::Forgejo, Some("https://forgejo.example.com"), "t").is_ok());
        assert!(build_client(SyncProvider::GitHub, None, "t").is_ok());
    }

    #[test]
    fn issue_state_round_trips_through_text() {
        assert_eq!(issue_state_from_text("open").unwrap(), IssueState::Open);
        assert_eq!(issue_state_from_text("closed").unwrap(), IssueState::Closed);
        assert_eq!(issue_state_to_text(IssueState::Open), "open");
        assert_eq!(issue_state_to_text(IssueState::Closed), "closed");
        assert!(issue_state_from_text("weird").is_err());
    }

    #[test]
    fn rfc3339_timestamps_round_trip() {
        let ts = parse_rfc3339("2024-01-02T03:04:05Z").unwrap();
        assert_eq!(format_rfc3339(ts).unwrap(), "2024-01-02T03:04:05Z");
    }

    #[test]
    fn a_pull_request_is_recognised_via_its_pull_request_field() {
        let issue: RawIssue = serde_json::from_str(
            r#"{"number":1,"title":"t","body":null,"html_url":"h","state":"open",
                "updated_at":"2024-01-01T00:00:00Z","pull_request":{"url":"x"}}"#,
        )
        .unwrap();
        assert!(issue.pull_request.is_some());

        let plain_issue: RawIssue = serde_json::from_str(
            r#"{"number":2,"title":"t","body":null,"html_url":"h","state":"open",
                "updated_at":"2024-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert!(plain_issue.pull_request.is_none());
    }
}
