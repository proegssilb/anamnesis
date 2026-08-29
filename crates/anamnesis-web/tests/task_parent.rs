//! The redesigned "Parent task" section (`templates/task.html`): a task
//! shows plainly, on its own page, whose checklist it belongs to; a parent
//! is picked by title search (`GET /tasks/{id}/parent-candidates`) rather
//! than by pasting a raw id; and clearing it is a single click.

mod support;

use axum::http::StatusCode;

use support::{TestApp, body_text, location_of};

/// Creates an area, a project in it, and returns the project's path — the
/// shared setup every test here needs before it can create tasks.
async fn new_project(app: &TestApp, cookie: Option<&str>) -> String {
    let area_path = location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Homesteading"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    location_of(
        &app.post_form(
            &format!("{area_path}/projects"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Renovation"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string()
}

async fn new_task(app: &TestApp, project_path: &str, title: &str, cookie: Option<&str>) -> String {
    location_of(
        &app.post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", title),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string()
}

#[tokio::test]
async fn a_task_with_no_parent_shows_no_checklist_badge_and_the_plain_hint() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let project_path = new_project(&app, cookie).await;
    let task_path = new_task(&app, &project_path, "Regrout the shower", cookie).await;

    let body = body_text(app.get(&task_path, cookie).await).await;
    assert!(!body.contains("checklist item of"));
    assert!(body.contains("Not part of another task"));
    assert!(body.contains("checklist."));
}

#[tokio::test]
async fn setting_a_parent_by_id_shows_up_as_a_badge_and_a_link_on_the_childs_own_page() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let project_path = new_project(&app, cookie).await;
    let parent_path = new_task(&app, &project_path, "Renovate the bathroom", cookie).await;
    let parent_id = parent_path.trim_start_matches("/tasks/");
    let child_path = new_task(&app, &project_path, "Regrout the shower", cookie).await;

    let set = app
        .post_form(
            &format!("{child_path}/parent"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("parent_task_id", parent_id),
            ],
            cookie,
        )
        .await;
    assert_eq!(set.status(), StatusCode::SEE_OTHER);

    let body = body_text(app.get(&child_path, cookie).await).await;
    assert!(
        body.contains("checklist item of") && body.contains("Renovate the bathroom"),
        "the child's own page must say whose checklist it is part of: {body}"
    );
    assert!(body.contains(&format!("href=\"/tasks/{parent_id}\"")));
    assert!(body.contains("Remove from checklist"));
}

#[tokio::test]
async fn clearing_a_parent_is_a_single_click_form() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let project_path = new_project(&app, cookie).await;
    let parent_path = new_task(&app, &project_path, "Renovate the bathroom", cookie).await;
    let parent_id = parent_path.trim_start_matches("/tasks/");
    let child_path = new_task(&app, &project_path, "Regrout the shower", cookie).await;

    app.post_form(
        &format!("{child_path}/parent"),
        &[
            ("csrf_token", support::DEV_CSRF_TOKEN),
            ("parent_task_id", parent_id),
        ],
        cookie,
    )
    .await;

    // The "Remove from checklist" button is a plain form carrying a blank
    // `parent_task_id` — the same clearing shape `SetParentForm`'s own doc
    // comment describes, just no longer requiring the user to blank out a
    // text field themselves.
    let clear = app
        .post_form(
            &format!("{child_path}/parent"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("parent_task_id", ""),
            ],
            cookie,
        )
        .await;
    assert_eq!(clear.status(), StatusCode::SEE_OTHER);

    let body = body_text(app.get(&child_path, cookie).await).await;
    assert!(!body.contains("checklist item of"));
    assert!(body.contains("Not part of another task"));
    assert!(body.contains("checklist."));
}

#[tokio::test]
async fn the_parent_candidate_search_finds_a_matching_task_by_title_and_excludes_itself() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let project_path = new_project(&app, cookie).await;
    let target_path = new_task(&app, &project_path, "Renovate the bathroom", cookie).await;
    let target_id = target_path.trim_start_matches("/tasks/");
    let searching_path = new_task(&app, &project_path, "Regrout the shower", cookie).await;

    let hits = body_text(
        app.get(
            &format!("{searching_path}/parent-candidates?q=Renovate"),
            cookie,
        )
        .await,
    )
    .await;
    assert!(
        hits.contains("Renovate the bathroom") && hits.contains(target_id),
        "the picker must find the other task by title: {hits}"
    );

    // A task can never be its own parent, so it must never appear as a
    // candidate for its own search — search on a word from its own title.
    let self_hits = body_text(
        app.get(
            &format!("{searching_path}/parent-candidates?q=Regrout"),
            cookie,
        )
        .await,
    )
    .await;
    assert!(
        !self_hits.contains("Use as parent"),
        "a task must not be offered as its own parent: {self_hits}"
    );
}

#[tokio::test]
async fn an_hx_candidate_search_gets_only_the_fragment_a_plain_request_gets_the_full_page() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let project_path = new_project(&app, cookie).await;
    let target_path = new_task(&app, &project_path, "Renovate the bathroom", cookie).await;
    let target_id = target_path.trim_start_matches("/tasks/");
    let searching_path = new_task(&app, &project_path, "Regrout the shower", cookie).await;

    let full = app
        .get(
            &format!("{searching_path}/parent-candidates?q=Renovate"),
            cookie,
        )
        .await;
    assert_eq!(full.status(), StatusCode::OK);
    let full_body = body_text(full).await;
    assert!(full_body.contains("<html") || full_body.contains("<!doctype html>"));
    assert!(full_body.contains(target_id));

    let fragment = app
        .get_hx(
            &format!("{searching_path}/parent-candidates?q=Renovate"),
            cookie,
        )
        .await;
    assert_eq!(fragment.status(), StatusCode::OK);
    let fragment_body = body_text(fragment).await;
    assert!(
        !fragment_body.contains("<html") && !fragment_body.contains("<!doctype html>"),
        "an HX-Request must get a bare candidates fragment: {fragment_body}"
    );
    assert!(fragment_body.contains(target_id));
}

#[tokio::test]
async fn a_user_without_a_view_grant_on_the_task_cannot_use_the_parent_picker() {
    // Dev-auth-bypass off, matching `f3_admin_routes.rs`'s own
    // "ungranted user" tests — under bypass, every cookie resolves to the
    // same dev user, so a *distinct* ungranted identity needs real sessions.
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", "admin-token"),
                ("title", "Homesteading"),
                ("description", ""),
            ],
            Some(&admin_cookie),
        )
        .await,
    )
    .to_string();
    let project_path = location_of(
        &app.post_form(
            &format!("{area_path}/projects"),
            &[
                ("csrf_token", "admin-token"),
                ("title", "Renovation"),
                ("description", ""),
            ],
            Some(&admin_cookie),
        )
        .await,
    )
    .to_string();
    let task_path = location_of(
        &app.post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", "admin-token"),
                ("title", "Regrout the shower"),
                ("description", ""),
            ],
            Some(&admin_cookie),
        )
        .await,
    )
    .to_string();

    let stranger_cookie = app.login_cookie_header("stranger", "stranger-token");
    let response = app
        .get(
            &format!("{task_path}/parent-candidates?q=Regrout"),
            Some(&stranger_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
