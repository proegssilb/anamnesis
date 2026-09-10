//! `tower::ServiceExt::oneshot` coverage for project sync with an external
//! issue tracker (issues #40/#41): `POST /projects/{id}/sync` (configure)
//! and `POST /projects/{id}/sync/now` (manual trigger), plus the imported-
//! comment origin annotation on the task page. Mirrors `tests/settings.rs`'s
//! structure.

mod support;

use axum::http::StatusCode;

use anamnesis_app::{CommentId, CommentOrigin, SyncProvider, configure_project_sync, import_comment};
use anamnesis_core::{ProjectId, TaskId};

use support::{TestApp, body_text, location_of};

fn project_id_of(project_path: &str) -> ProjectId {
    ProjectId::new(
        project_path
            .trim_start_matches("/projects/")
            .parse()
            .unwrap(),
    )
}

fn task_id_of(task_path: &str) -> TaskId {
    TaskId::new(task_path.trim_start_matches("/tasks/").parse().unwrap())
}

/// Builds the full `ConfigureSyncForm` field set -- the form is
/// all-or-nothing, like `/settings`'s, so every test restates every field
/// even when only one is under test.
fn sync_form<'a>(
    csrf: &'a str,
    provider: &'a str,
    owner: &'a str,
    repo: &'a str,
    token: &'a str,
) -> Vec<(&'a str, &'a str)> {
    vec![
        ("csrf_token", csrf),
        ("provider", provider),
        ("base_url", ""),
        ("owner", owner),
        ("repo", repo),
        ("token", token),
        ("auto_import_new_issues", "1"),
        ("auto_push_new_tasks", "1"),
        ("enabled", "1"),
    ]
}

#[tokio::test]
async fn post_sync_by_a_non_admin_is_forbidden() {
    let app = TestApp::with_sync_encryption_key(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let stranger_cookie = app.login_cookie_header("stranger", "stranger-token");
    let (_, project_path) =
        support::new_area_with_project(&app, "Home", "Repaint", "admin-token", Some(&admin_cookie))
            .await;
    let project_id = project_id_of(&project_path);

    let response = app
        .post_form(
            &format!("/projects/{project_id}/sync"),
            &sync_form("stranger-token", "github", "octocat", "hello-world", "secret-pat"),
            Some(&stranger_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn post_sync_without_a_valid_csrf_token_is_rejected() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let (_, project_path) =
        support::new_area_with_project(&app, "Home", "Repaint", "admin-token", Some(&admin_cookie))
            .await;
    let project_id = project_id_of(&project_path);

    let response = app
        .post_form(
            &format!("/projects/{project_id}/sync"),
            &sync_form("wrong-token", "github", "octocat", "hello-world", "secret-pat"),
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn configuring_github_sync_without_a_base_url_succeeds_and_the_project_page_reflects_it() {
    let app = TestApp::with_sync_encryption_key(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let (_, project_path) =
        support::new_area_with_project(&app, "Home", "Repaint", "admin-token", Some(&admin_cookie))
            .await;
    let project_id = project_id_of(&project_path);

    let response = app
        .post_form(
            &format!("/projects/{project_id}/sync"),
            &sync_form("admin-token", "github", "octocat", "hello-world", "secret-pat"),
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(location_of(&response).contains("#project-settings"));

    let page = body_text(app.get(&project_path, Some(&admin_cookie)).await).await;
    assert!(page.contains("Enabled"));
    assert!(page.contains("syncing with github: octocat/hello-world."));
}

#[tokio::test]
async fn configuring_forgejo_sync_without_a_base_url_is_rejected() {
    let app = TestApp::with_sync_encryption_key(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let (_, project_path) =
        support::new_area_with_project(&app, "Home", "Repaint", "admin-token", Some(&admin_cookie))
            .await;
    let project_id = project_id_of(&project_path);

    let response = app
        .post_form(
            &format!("/projects/{project_id}/sync"),
            &sync_form("admin-token", "forgejo", "octocat", "hello-world", "secret-pat"),
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_text(response).await;
    assert!(body.contains("a Forgejo instance URL is required"));
}

#[tokio::test]
async fn a_blank_token_on_edit_keeps_the_stored_ciphertext() {
    let app = TestApp::with_sync_encryption_key(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let (_, project_path) =
        support::new_area_with_project(&app, "Home", "Repaint", "admin-token", Some(&admin_cookie))
            .await;
    let project_id = project_id_of(&project_path);

    let first = app
        .post_form(
            &format!("/projects/{project_id}/sync"),
            &sync_form("admin-token", "github", "octocat", "hello-world", "the-real-secret"),
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(first.status(), StatusCode::SEE_OTHER);

    // Edit again, leaving the token field blank -- this must keep the
    // original secret rather than clearing or corrupting it.
    let second = app
        .post_form(
            &format!("/projects/{project_id}/sync"),
            &sync_form("admin-token", "github", "octocat", "renamed-repo", ""),
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(second.status(), StatusCode::SEE_OTHER);

    let stored = app
        .state
        .project_sync_configs
        .load(project_id)
        .await
        .unwrap()
        .expect("a config exists");
    assert_eq!(stored.repo, "renamed-repo");
    let cipher = app.state.token_cipher.as_ref().expect("cipher configured");
    assert_eq!(
        cipher.decrypt(&stored.encrypted_token).unwrap(),
        "the-real-secret"
    );
}

#[tokio::test]
async fn trigger_sync_now_with_no_encryption_key_configured_returns_a_clear_4xx_not_a_panic() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let (_, project_path) =
        support::new_area_with_project(&app, "Home", "Repaint", "admin-token", Some(&admin_cookie))
            .await;
    let project_id = project_id_of(&project_path);

    // No `token_cipher` is wired into this `TestApp` (matching a deployment
    // with `ANAMNESIS_SYNC_ENCRYPTION_KEY` unset), so the config has to be
    // seeded directly through the repository -- going through the HTTP form
    // would itself be refused, for the same reason.
    let now = app.state.clock.now();
    let config = configure_project_sync(
        project_id,
        SyncProvider::GitHub,
        None,
        "octocat",
        "hello-world",
        vec![1, 2, 3],
        true,
        true,
        now,
    )
    .unwrap();
    app.state
        .project_sync_configs
        .upsert(&config)
        .await
        .unwrap();

    let response = app
        .post_form(
            &format!("/projects/{project_id}/sync/now"),
            &[("csrf_token", "admin-token")],
            Some(&admin_cookie),
        )
        .await;
    assert!(
        response.status().is_client_error(),
        "expected a clear 4xx, got {}",
        response.status()
    );
}

#[tokio::test]
async fn an_imported_comment_renders_its_origin_but_a_plain_comment_does_not() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let (_, project_path) =
        support::new_area_with_project(&app, "Home", "Repaint", "admin-token", Some(&admin_cookie))
            .await;
    let task_path = support::new_task_as(
        &app,
        &project_path,
        "Regrout the shower",
        "admin-token",
        Some(&admin_cookie),
    )
    .await;
    let task_id = task_id_of(&task_path);

    let plain = app
        .post_form(
            &format!("{task_path}/comments"),
            &[("csrf_token", "admin-token"), ("body", "looks good to me")],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(plain.status(), StatusCode::SEE_OTHER);

    let now = app.state.clock.now();
    let origin = CommentOrigin {
        provider: SyncProvider::GitHub,
        external_comment_id: 42,
        external_url: "https://github.com/octocat/hello-world/issues/1#issuecomment-42"
            .to_string(),
        external_author_display: "octocat".to_string(),
    };
    let imported = import_comment(
        CommentId::new(app.state.id_gen.next()),
        task_id,
        "fixed upstream in a follow-up commit",
        origin,
        now,
    )
    .unwrap();
    app.state.comments.insert(&imported).await.unwrap();

    let page = body_text(app.get(&task_path, Some(&admin_cookie)).await).await;
    assert!(page.contains("looks good to me"));
    assert!(page.contains("fixed upstream in a follow-up commit"));
    assert!(page.contains("Originally posted on GitHub"));
    // The URL is HTML-escaped by the template engine (`/` becomes `&#x2f;`),
    // so match on the distinctive, unescaped tail rather than the literal
    // URL string.
    assert!(page.contains("issuecomment-42"));

    // Only the imported comment's own block carries the origin annotation --
    // the plain comment renders first (creation order), so the origin
    // marker must appear only after it, not wrapped around it too.
    let plain_idx = page.find("looks good to me").unwrap();
    let origin_idx = page.find("Originally posted on GitHub").unwrap();
    assert!(plain_idx < origin_idx);
}
