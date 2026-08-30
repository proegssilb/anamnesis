//! Title-search picker for the "Add relationship" modal
//! (`templates/task.html`): `GET /tasks/{id}/relationship-candidates` finds
//! a target by title, and selecting one round-trips `?rel_to=<id>` back to
//! the task page, where it prefills the relationship form's `to_task_id`.

mod support;

use axum::http::StatusCode;

use support::{TestApp, body_text, location_of};

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
async fn the_relationship_candidate_search_finds_a_matching_task_by_title_and_excludes_itself() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let project_path = new_project(&app, cookie).await;
    let target_path = new_task(&app, &project_path, "Renovate the bathroom", cookie).await;
    let target_id = target_path.trim_start_matches("/tasks/");
    let searching_path = new_task(&app, &project_path, "Regrout the shower", cookie).await;

    let hits = body_text(
        app.get(
            &format!("{searching_path}/relationship-candidates?q=Renovate"),
            cookie,
        )
        .await,
    )
    .await;
    assert!(
        hits.contains("Renovate the bathroom") && hits.contains(target_id),
        "the picker must find the other task by title: {hits}"
    );

    // A task can never relate to itself, so it must never appear as a
    // candidate for its own search — search on a word from its own title.
    let self_hits = body_text(
        app.get(
            &format!("{searching_path}/relationship-candidates?q=Regrout"),
            cookie,
        )
        .await,
    )
    .await;
    assert!(
        !self_hits.contains("Select"),
        "a task must not be offered as a relationship target for itself: {self_hits}"
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
            &format!("{searching_path}/relationship-candidates?q=Renovate"),
            cookie,
        )
        .await;
    assert_eq!(full.status(), StatusCode::OK);
    let full_body = body_text(full).await;
    assert!(full_body.contains("<html") || full_body.contains("<!doctype html>"));
    assert!(full_body.contains(target_id));

    let fragment = app
        .get_hx(
            &format!("{searching_path}/relationship-candidates?q=Renovate"),
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
async fn selecting_a_candidate_prefills_the_relationship_form_and_the_relationship_still_creates() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let project_path = new_project(&app, cookie).await;
    let target_path = new_task(&app, &project_path, "Renovate the bathroom", cookie).await;
    let target_id = target_path.trim_start_matches("/tasks/");
    let task_path = new_task(&app, &project_path, "Regrout the shower", cookie).await;

    let prefilled = body_text(
        app.get(&format!("{task_path}?rel_to={target_id}"), cookie)
            .await,
    )
    .await;
    assert!(
        prefilled.contains("Selected: Renovate the bathroom"),
        "the modal must show the selected task: {prefilled}"
    );
    assert!(prefilled.contains(&format!("value=\"{target_id}\"")));

    // An unparseable or unknown id degrades to "no prefill" rather than
    // erroring the whole page.
    let garbage = app
        .get(&format!("{task_path}?rel_to=not-a-uuid"), cookie)
        .await;
    assert_eq!(garbage.status(), StatusCode::OK);
    let missing = app
        .get(
            &format!("{task_path}?rel_to=00000000-0000-0000-0000-000000000000"),
            cookie,
        )
        .await;
    assert_eq!(missing.status(), StatusCode::OK);

    let created = app
        .post_form(
            &format!("{task_path}/relationships"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("to_task_id", target_id),
                ("kind", "blocks"),
            ],
            cookie,
        )
        .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);

    let body = body_text(app.get(&task_path, cookie).await).await;
    assert!(body.contains("Renovate the bathroom"));
}

#[tokio::test]
async fn a_user_without_a_view_grant_on_the_task_cannot_use_the_relationship_picker() {
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
            &format!("{task_path}/relationship-candidates?q=Regrout"),
            Some(&stranger_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
