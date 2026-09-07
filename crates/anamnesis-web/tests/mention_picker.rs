//! `tower::ServiceExt::oneshot` coverage for the `@`-mention picker's data
//! feed (issue #44 follow-up): `crate::handlers::format::
//! mentionable_users_json`, embedded on the task and project pages as a
//! `<script type="application/json">` block `static/app.js` reads.

mod support;

use axum::http::StatusCode;

use support::{DEV_CSRF_TOKEN, TestApp, body_text};

async fn create_area(app: &TestApp, cookie: &str, csrf: &str, title: &str) -> String {
    support::new_area(app, title, csrf, Some(cookie)).await
}

async fn create_project(
    app: &TestApp,
    cookie: &str,
    csrf: &str,
    area_path: &str,
    title: &str,
) -> String {
    support::new_project_in(app, area_path, title, csrf, Some(cookie)).await
}

#[tokio::test]
async fn a_direct_project_member_appears_in_the_project_pages_mention_data() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = create_area(&app, &admin_cookie, "admin-token", "Home").await;
    let project_path =
        create_project(&app, &admin_cookie, "admin-token", &area_path, "Repaint").await;
    app.post_form(
        &format!("{project_path}/members"),
        &[
            ("csrf_token", "admin-token"),
            ("user_id", "bob"),
            ("role", "member"),
        ],
        Some(&admin_cookie),
    )
    .await;

    let body = body_text(app.get(&project_path, Some(&admin_cookie)).await).await;
    assert!(body.contains(r#"id="project-mention-users""#));
    assert!(body.contains(r#""id":"bob""#));
}

#[tokio::test]
async fn a_system_admin_grant_does_not_leak_into_the_mention_data() {
    // System Admin is deliberately excluded from list_mentionable_users
    // (its own doc comment): granting it must not make the admin
    // themselves show up as a mention candidate through this weaker gate.
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = create_area(&app, &admin_cookie, "admin-token", "Home").await;
    let project_path =
        create_project(&app, &admin_cookie, "admin-token", &area_path, "Repaint").await;

    let body = body_text(app.get(&project_path, Some(&admin_cookie)).await).await;
    assert!(body.contains(r#"id="project-mention-users""#));
    assert!(!body.contains(r#""id":"admin""#));
}

#[tokio::test]
async fn the_tasks_own_page_carries_its_projects_mention_data_too() {
    let app = TestApp::new(true).await;
    let (_, project_path) =
        support::new_active_project(&app, "Home", "Kitchen remodel", None).await;
    app.post_form(
        &format!("{project_path}/members"),
        &[
            ("csrf_token", DEV_CSRF_TOKEN),
            ("user_id", "carol"),
            ("role", "member"),
        ],
        None,
    )
    .await;
    let task_path =
        support::new_task_as(&app, &project_path, "Tile the floor", DEV_CSRF_TOKEN, None).await;

    let response = app.get(&task_path, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(body.contains(r#"id="task-mention-users""#));
    assert!(body.contains(r#""id":"carol""#));
}
