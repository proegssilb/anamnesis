//! `tower::ServiceExt::oneshot` coverage for issue #34: "Allow bulk-adding
//! areas, projects, and tasks" — everything used to go in one at a time,
//! which is painful for initial setup or transcribing a project plan
//! already fully formed in your head. `crate::handlers::areas::{
//! bulk_create_areas_impl, bulk_create_projects_impl}` and
//! `crate::handlers::projects::bulk_create_tasks_impl` each accept a
//! textarea of newline-separated titles and create one item per line.

mod support;

use axum::http::StatusCode;

use support::{TestApp, body_text, new_area, new_area_with_project, new_project_in};

#[tokio::test]
async fn bulk_adding_areas_creates_one_per_pasted_line() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let response = app
        .post_form(
            "/areas/bulk",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("titles", "Home\nHealth\nFinances"),
            ],
            cookie,
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let body = body_text(app.get("/areas", cookie).await).await;
    assert!(body.contains("Home"), "{body}");
    assert!(body.contains("Health"), "{body}");
    assert!(body.contains("Finances"), "{body}");
}

#[tokio::test]
async fn bulk_adding_projects_creates_one_per_pasted_line_in_the_right_area() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (area_path, _) = new_area_with_project(
        &app,
        "Home Ops",
        "Existing project",
        support::DEV_CSRF_TOKEN,
        cookie,
    )
    .await;
    let area_id = area_path.trim_start_matches("/areas/");

    let response = app
        .post_form(
            &format!("/areas/{area_id}/projects/bulk"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("titles", "Repaint the shed\nClean the gutters"),
            ],
            cookie,
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let body = body_text(app.get(&area_path, cookie).await).await;
    assert!(body.contains("Repaint the shed"), "{body}");
    assert!(body.contains("Clean the gutters"), "{body}");
}

#[tokio::test]
async fn bulk_adding_tasks_creates_one_per_pasted_line_in_the_right_project() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let area_path = new_area(&app, "Home Ops", support::DEV_CSRF_TOKEN, cookie).await;
    let project_path = new_project_in(
        &app,
        &area_path,
        "Repaint the shed",
        support::DEV_CSRF_TOKEN,
        cookie,
    )
    .await;
    let project_id = project_path.trim_start_matches("/projects/");

    let response = app
        .post_form(
            &format!("/projects/{project_id}/tasks/bulk"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("titles", "Buy primer\nSand the trim\nPaint the door"),
            ],
            cookie,
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let body = body_text(app.get(&project_path, cookie).await).await;
    assert!(body.contains("Buy primer"), "{body}");
    assert!(body.contains("Sand the trim"), "{body}");
    assert!(body.contains("Paint the door"), "{body}");
}

#[tokio::test]
async fn blank_lines_are_skipped_and_surrounding_whitespace_is_trimmed() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let response = app
        .post_form(
            "/areas/bulk",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("titles", "  Home  \n\n   \nHealth\n"),
            ],
            cookie,
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let body = body_text(app.get("/areas", cookie).await).await;
    assert!(
        body.contains(">Home<"),
        "must trim surrounding whitespace: {body}"
    );
    assert!(body.contains(">Health<"), "{body}");
    assert!(
        !body.contains("<h2></h2>"),
        "blank lines must not create empty-titled areas: {body}"
    );
}

#[tokio::test]
async fn a_rejected_title_does_not_lose_the_rest_of_the_paste() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let too_long = "x".repeat(201);

    let response = app
        .post_form(
            "/areas/bulk",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("titles", &format!("Home\n{too_long}\nHealth")),
            ],
            cookie,
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_text(response).await;
    assert!(
        body.contains("Home") && body.contains("Health"),
        "the valid titles either side of the bad one must still have been created: {body}"
    );
}

#[tokio::test]
async fn bulk_add_without_a_valid_csrf_token_is_rejected() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let response = app
        .post_form(
            "/areas/bulk",
            &[
                ("csrf_token", "not-the-real-token"),
                ("titles", "Sneaky area"),
            ],
            cookie,
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let body = body_text(app.get("/areas", cookie).await).await;
    assert!(!body.contains("Sneaky area"));
}

#[tokio::test]
async fn an_empty_paste_is_rejected_with_no_areas_created() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let response = app
        .post_form(
            "/areas/bulk",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("titles", "\n   \n"),
            ],
            cookie,
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_text(response).await;
    assert!(body.contains("at least one title"), "{body}");
}
