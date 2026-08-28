//! `tower::ServiceExt::oneshot` coverage for the system-wide Projects data
//! grid (`GET /projects`): the default filter hides archived and Complete
//! projects, the explicit `status`/`archived` overrides reveal them, the
//! `area` filter narrows to one area, and — exactly like the Areas grid
//! (`tests/auth_and_access.rs`'s `the_area_grid_is_empty_for_a_user_with_no_grants_rather_than_forbidden`)
//! — a user with no grant on an area never sees its projects here, no
//! matter which filters they pass.

mod support;

use axum::http::StatusCode;

use support::{TestApp, body_text, location_of};

async fn create_area(app: &TestApp, cookie: &str, csrf: &str, title: &str) -> String {
    let response = app
        .post_form(
            "/areas",
            &[("csrf_token", csrf), ("title", title), ("description", "")],
            Some(cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    location_of(&response).to_string()
}

async fn create_project(
    app: &TestApp,
    cookie: &str,
    csrf: &str,
    area_path: &str,
    title: &str,
) -> String {
    let response = app
        .post_form(
            &format!("{area_path}/projects"),
            &[("csrf_token", csrf), ("title", title), ("description", "")],
            Some(cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    location_of(&response).to_string()
}

async fn set_status(app: &TestApp, cookie: &str, csrf: &str, project_path: &str, status: &str) {
    let response = app
        .post_form(
            &format!("{project_path}/status"),
            &[("csrf_token", csrf), ("status", status)],
            Some(cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn default_listing_hides_archived_and_complete_projects() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let cookie = app.login_cookie_header("admin", "admin-token");
    let csrf = "admin-token";

    let area_path = create_area(&app, &cookie, csrf, "Home Ops").await;
    let pending_path = create_project(&app, &cookie, csrf, &area_path, "Fix the fence").await;
    let active_path = create_project(&app, &cookie, csrf, &area_path, "Repaint the shed").await;
    let complete_path = create_project(&app, &cookie, csrf, &area_path, "Gutter cleaning").await;

    set_status(&app, &cookie, csrf, &active_path, "active").await;
    set_status(&app, &cookie, csrf, &complete_path, "active").await;
    set_status(&app, &cookie, csrf, &complete_path, "complete").await;

    let body = body_text(app.get("/projects", Some(&cookie)).await).await;
    assert!(body.contains("Fix the fence"), "pending must show: {body}");
    assert!(
        body.contains("Repaint the shed"),
        "active must show: {body}"
    );
    assert!(
        !body.contains("Gutter cleaning"),
        "complete must be hidden by default: {body}"
    );

    // Archive the pending project — it must also drop out of the default
    // view even though its status alone would otherwise pass.
    let archive = app
        .post_form(
            &format!("{pending_path}/archive"),
            &[("csrf_token", csrf)],
            Some(&cookie),
        )
        .await;
    assert_eq!(archive.status(), StatusCode::SEE_OTHER);

    let body = body_text(app.get("/projects", Some(&cookie)).await).await;
    assert!(
        !body.contains("Fix the fence"),
        "an archived project must be hidden by default: {body}"
    );
}

#[tokio::test]
async fn status_all_and_the_archived_checkbox_reveal_everything() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let cookie = app.login_cookie_header("admin", "admin-token");
    let csrf = "admin-token";

    let area_path = create_area(&app, &cookie, csrf, "Home Ops").await;
    let complete_path = create_project(&app, &cookie, csrf, &area_path, "Gutter cleaning").await;
    set_status(&app, &cookie, csrf, &complete_path, "active").await;
    set_status(&app, &cookie, csrf, &complete_path, "complete").await;

    let archived_path = create_project(&app, &cookie, csrf, &area_path, "Old shed teardown").await;
    app.post_form(
        &format!("{archived_path}/archive"),
        &[("csrf_token", csrf)],
        Some(&cookie),
    )
    .await;

    let all_statuses = body_text(app.get("/projects?status=all", Some(&cookie)).await).await;
    assert!(
        all_statuses.contains("Gutter cleaning"),
        "status=all must include Complete projects: {all_statuses}"
    );
    assert!(
        !all_statuses.contains("Old shed teardown"),
        "status=all alone must still hide archived projects: {all_statuses}"
    );

    let with_archived = body_text(app.get("/projects?archived=1", Some(&cookie)).await).await;
    assert!(
        with_archived.contains("Old shed teardown"),
        "the archived checkbox must reveal archived projects: {with_archived}"
    );
}

#[tokio::test]
async fn the_area_filter_narrows_to_one_area() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let cookie = app.login_cookie_header("admin", "admin-token");
    let csrf = "admin-token";

    let home_path = create_area(&app, &cookie, csrf, "Home Ops").await;
    let health_path = create_area(&app, &cookie, csrf, "Personal Life").await;
    create_project(&app, &cookie, csrf, &home_path, "Fix the fence").await;
    create_project(&app, &cookie, csrf, &health_path, "Marathon training").await;

    let home_id = home_path.trim_start_matches("/areas/");
    let filtered = body_text(
        app.get(&format!("/projects?area={home_id}"), Some(&cookie))
            .await,
    )
    .await;
    assert!(filtered.contains("Fix the fence"), "{filtered}");
    assert!(!filtered.contains("Marathon training"), "{filtered}");
}

#[tokio::test]
async fn a_user_with_no_grant_on_the_area_never_sees_its_projects() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");

    let area_path = create_area(&app, &admin_cookie, "admin-token", "Home Ops").await;
    create_project(
        &app,
        &admin_cookie,
        "admin-token",
        &area_path,
        "Fix the fence",
    )
    .await;

    let stranger_cookie = app.login_cookie_header("stranger", "stranger-token");
    let response = app
        .get("/projects?status=all&archived=1", Some(&stranger_cookie))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(
        !body.contains("Fix the fence"),
        "an ungranted user must not see another area's project, under any filter: {body}"
    );
}
