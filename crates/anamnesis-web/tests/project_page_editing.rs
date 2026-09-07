//! `tower::ServiceExt::oneshot` coverage for issue #42: click-to-edit title,
//! description, and status pill directly on the project page
//! (`crate::handlers::projects::{edit_project_title_handler,
//! edit_project_description_handler}`, and the project page's own POST to
//! the pre-existing `/projects/{id}/status` route).

mod support;

use axum::http::StatusCode;

use support::{TestApp, body_text, location_of, new_area, new_project_in};

async fn create_area(app: &TestApp, cookie: &str, csrf: &str, title: &str) -> String {
    new_area(app, title, csrf, Some(cookie)).await
}

async fn create_project(
    app: &TestApp,
    cookie: &str,
    csrf: &str,
    area_path: &str,
    title: &str,
) -> String {
    new_project_in(app, area_path, title, csrf, Some(cookie)).await
}

#[tokio::test]
async fn a_project_admin_can_rename_and_redescribe_the_project_in_place() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = create_area(&app, &admin_cookie, "admin-token", "Home").await;
    let project_path =
        create_project(&app, &admin_cookie, "admin-token", &area_path, "Repaint").await;

    let renamed = app
        .post_form(
            &format!("{project_path}/title"),
            &[("csrf_token", "admin-token"), ("title", "Repaint the shed")],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(renamed.status(), StatusCode::OK);
    let body = body_text(renamed).await;
    assert!(body.contains("Repaint the shed"));

    let described = app
        .post_form(
            &format!("{project_path}/description"),
            &[
                ("csrf_token", "admin-token"),
                ("description", "Two coats, weather permitting"),
            ],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(described.status(), StatusCode::OK);
    let body = body_text(described).await;
    assert!(body.contains("Two coats, weather permitting"));
}

#[tokio::test]
async fn transitioning_status_from_the_project_page_redirects_back_to_it_not_the_area() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = create_area(&app, &admin_cookie, "admin-token", "Home").await;
    let project_path =
        create_project(&app, &admin_cookie, "admin-token", &area_path, "Repaint").await;

    let response = app
        .post_form(
            &format!("{project_path}/status"),
            &[
                ("csrf_token", "admin-token"),
                ("status", "active"),
                ("return_to", "project"),
            ],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location_of(&response), project_path);
}

#[tokio::test]
async fn a_plain_member_cannot_rename_the_project() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = create_area(&app, &admin_cookie, "admin-token", "Home").await;
    let project_path =
        create_project(&app, &admin_cookie, "admin-token", &area_path, "Repaint").await;

    let bob_cookie = app.login_cookie_header("bob", "bob-token");
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

    let response = app
        .post_form(
            &format!("{project_path}/title"),
            &[("csrf_token", "bob-token"), ("title", "Hijacked")],
            Some(&bob_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
