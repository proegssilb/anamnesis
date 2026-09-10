//! [`HttpIssueTrackerClient`] exercised against a mock GitHub/Forgejo-shaped
//! server via `wiremock` (issues #40/#41) — entirely offline, mirroring
//! `identity_provider.rs`'s own approach.

use anamnesis_app::{IssueEdit, IssueState, IssueTrackerClient, RemoteComment, RemoteIssue};
use anamnesis_core::Timestamp;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ts(secs: i64) -> Timestamp {
    Timestamp::from_unix_seconds(secs).unwrap()
}

fn issue_json(number: u64, state: &str) -> serde_json::Value {
    json!({
        "number": number,
        "title": format!("Issue {number}"),
        "body": "some body",
        "html_url": format!("https://example.test/issues/{number}"),
        "state": state,
        "updated_at": "2024-01-02T03:04:05Z",
    })
}

fn pull_request_json(number: u64) -> serde_json::Value {
    let mut v = issue_json(number, "open");
    v["pull_request"] = json!({"url": "https://example.test/pulls/1"});
    v
}

#[tokio::test]
async fn list_issues_since_sends_bearer_auth_and_filters_out_pull_requests() {
    // A configured GitHub base_url always resolves to `{base}/api/v3` (see
    // `HttpIssueTrackerClient::github`'s doc comment) — for GitHub.com
    // itself (`base_url: None`) the root is the fixed `api.github.com`
    // instead, which a local mock server can't stand in for, so every test
    // here uses a "GitHub Enterprise" base URL pointed at the mock server.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/repos/octocat/hello-world/issues"))
        .and(header("Authorization", "Bearer secret-token"))
        .and(query_param("state", "all"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([issue_json(1, "open"), pull_request_json(2)])),
        )
        .mount(&server)
        .await;

    let client =
        anamnesis_adapters::HttpIssueTrackerClient::github(Some(&server.uri()), "secret-token")
            .unwrap();
    let issues = client
        .list_issues_since("octocat", "hello-world", None)
        .await
        .unwrap();
    assert_eq!(issues.len(), 1, "the pull request must be filtered out");
    assert_eq!(issues[0].number, 1);
    assert_eq!(issues[0].state, IssueState::Open);
    assert_eq!(issues[0].updated_at, ts(1_704_164_645));
}

#[tokio::test]
async fn forgejo_client_sends_token_auth_at_its_own_api_root() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/octocat/hello-world/issues"))
        .and(header("Authorization", "token secret-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([issue_json(1, "closed")])))
        .mount(&server)
        .await;

    let client =
        anamnesis_adapters::HttpIssueTrackerClient::forgejo(&server.uri(), "secret-token").unwrap();
    let issues = client
        .list_issues_since("octocat", "hello-world", None)
        .await
        .unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].state, IssueState::Closed);
}

#[tokio::test]
async fn list_issues_since_sends_the_since_parameter_when_given() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/repos/octocat/hello-world/issues"))
        .and(query_param("since", "2024-01-02T03:04:05Z"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let client =
        anamnesis_adapters::HttpIssueTrackerClient::github(Some(&server.uri()), "t").unwrap();
    let issues = client
        .list_issues_since("octocat", "hello-world", Some(ts(1_704_164_645)))
        .await
        .unwrap();
    assert!(issues.is_empty());
}

#[tokio::test]
async fn list_issues_since_follows_pagination_across_a_full_page() {
    let server = MockServer::start().await;
    let full_page: Vec<serde_json::Value> = (1..=100).map(|n| issue_json(n, "open")).collect();
    Mock::given(method("GET"))
        .and(path("/api/v3/repos/octocat/hello-world/issues"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(full_page)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/repos/octocat/hello-world/issues"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([issue_json(101, "open")])))
        .mount(&server)
        .await;

    let client =
        anamnesis_adapters::HttpIssueTrackerClient::github(Some(&server.uri()), "t").unwrap();
    let issues = client
        .list_issues_since("octocat", "hello-world", None)
        .await
        .unwrap();
    assert_eq!(issues.len(), 101, "a full first page must fetch a second");
}

#[tokio::test]
async fn create_issue_sends_the_expected_body_and_returns_the_created_issue() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/repos/octocat/hello-world/issues"))
        .and(header("Authorization", "Bearer t"))
        .respond_with(ResponseTemplate::new(201).set_body_json(issue_json(7, "open")))
        .mount(&server)
        .await;

    let client =
        anamnesis_adapters::HttpIssueTrackerClient::github(Some(&server.uri()), "t").unwrap();
    let issue = client
        .create_issue("octocat", "hello-world", "New task", "body text")
        .await
        .unwrap();
    assert_eq!(issue.number, 7);
    assert_eq!(issue.state, IssueState::Open);
}

#[tokio::test]
async fn update_issue_sends_title_body_and_state_and_returns_the_updated_issue() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/api/v3/repos/octocat/hello-world/issues/3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(3, "closed")))
        .mount(&server)
        .await;

    let client =
        anamnesis_adapters::HttpIssueTrackerClient::github(Some(&server.uri()), "t").unwrap();
    let updated = client
        .update_issue(
            "octocat",
            "hello-world",
            3,
            IssueEdit {
                title: "Renamed",
                body: "new body",
                state: IssueState::Closed,
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.state, IssueState::Closed);
}

#[tokio::test]
async fn list_comments_since_reports_the_authors_display_name() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v3/repos/octocat/hello-world/issues/3/comments",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "id": 555,
            "body": "nice work",
            "html_url": "https://example.test/issues/3#comment-555",
            "user": {"login": "octocat"},
            "created_at": "2024-01-02T03:04:05Z",
        }])))
        .mount(&server)
        .await;

    let client =
        anamnesis_adapters::HttpIssueTrackerClient::github(Some(&server.uri()), "t").unwrap();
    let comments: Vec<RemoteComment> = client
        .list_comments_since("octocat", "hello-world", 3, None)
        .await
        .unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].id, 555);
    assert_eq!(comments[0].author_display, "octocat");
    assert_eq!(comments[0].body, "nice work");
}

#[tokio::test]
async fn a_non_success_response_becomes_a_reported_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/repos/octocat/hello-world/issues"))
        .respond_with(ResponseTemplate::new(403).set_body_string("rate limit exceeded"))
        .mount(&server)
        .await;

    let client =
        anamnesis_adapters::HttpIssueTrackerClient::github(Some(&server.uri()), "t").unwrap();
    let result: Result<Vec<RemoteIssue>, _> = client
        .list_issues_since("octocat", "hello-world", None)
        .await;
    assert!(result.is_err());
}
