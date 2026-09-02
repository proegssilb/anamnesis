//! `tower::ServiceExt::oneshot` coverage for the project page's drag-and-drop
//! endpoints (`docs/DOMAIN.md` §8): `POST /projects/{id}/tasks/{task_id}/raise`
//! and `.../drop`, each reached two ways — an htmx drag (`HX-Request: true`,
//! exercised here the same way `static/app.js`'s `htmx.ajax` call would) and
//! the plain-form fallback (`_project_task_list.html`'s no-JS buttons) — plus
//! the WIP-limit and bounce-accounting behaviour they share with the global
//! task board's own raise/drop endpoints (`tests/reposition.rs`,
//! `tests/flow.rs`).

mod support;

use axum::http::StatusCode;

use anamnesis_app::BoardQuery;
use anamnesis_web::bootstrap::DEFAULT_TODO_WIP_LIMIT;
use support::{TestApp, body_text, location_of};

/// Creates an area and an active-enough project, and returns the project's
/// path plus the board's three bootstrapped column ids (`To-Do`, `Doing`,
/// `Done`), in that order.
async fn setup_project(app: &TestApp) -> (String, Vec<anamnesis_core::ColumnId>) {
    let cookie: Option<&str> = None;
    let project_path = support::new_project(app, cookie).await;
    let columns = app
        .store
        .columns_with_items()
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.column.id)
        .collect();
    (project_path, columns)
}

async fn new_task_in(app: &TestApp, project_path: &str, title: &str) -> String {
    support::new_task(app, project_path, title, None).await
}

/// Builds the project-scoped raise/drop URL under test here — as opposed to
/// the *global* `/tasks/{id}/raise` (`/drop`) endpoints `tests/reposition.rs`
/// and `tests/flow.rs` already cover, which `task_path` (as returned by
/// `new_task_in`, e.g. `/tasks/{uuid}`) would otherwise resolve to if
/// `action` were appended to it directly.
fn project_task_url(project_path: &str, task_path: &str, action: &str) -> String {
    let task_id = task_path
        .strip_prefix("/tasks/")
        .expect("new_task_in returns a `/tasks/{id}` path");
    format!("{project_path}/tasks/{task_id}/{action}")
}

#[tokio::test]
async fn raising_via_the_plain_form_moves_the_task_onto_the_board() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (project_path, columns) = setup_project(&app).await;
    let task_path = new_task_in(&app, &project_path, "Regrout the shower").await;

    let response = app
        .post_form(
            &project_task_url(&project_path, &task_path, "raise"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location_of(&response), project_path);

    let board = app.store.columns_with_items().await.unwrap();
    let todo = board.iter().find(|c| c.column.id == columns[0]).unwrap();
    assert_eq!(
        todo.items.len(),
        1,
        "the project raise endpoint always raises onto the entry (first) column"
    );

    let task_page = body_text(app.get(&task_path, cookie).await).await;
    assert!(task_page.contains("on the board"));
}

#[tokio::test]
async fn raising_via_hx_returns_both_oob_lists_with_the_task_moved() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (project_path, _columns) = setup_project(&app).await;
    let task_path = new_task_in(&app, &project_path, "Regrout the shower").await;

    let response = app
        .post_form_hx(
            &project_task_url(&project_path, &task_path, "raise"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "an HX-Request raise returns the fragment directly, not a redirect"
    );
    let body = body_text(response).await;
    assert!(
        !body.contains("<!doctype html>") && !body.contains("<html"),
        "an HX-Request must get bare fragments, not the page shell: {body}"
    );
    assert!(body.contains("hx-swap-oob=\"true\""));
    assert!(body.contains("id=\"on-board-list\""));
    assert!(body.contains("id=\"below-list\""));
    assert!(
        body.contains("Regrout the shower"),
        "the task must now appear in the returned lists: {body}"
    );
}

#[tokio::test]
async fn dropping_via_the_plain_form_moves_the_task_back_below_the_horizon() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (project_path, _columns) = setup_project(&app).await;
    let task_path = new_task_in(&app, &project_path, "Regrout the shower").await;
    app.post_form(
        &project_task_url(&project_path, &task_path, "raise"),
        &[("csrf_token", support::DEV_CSRF_TOKEN)],
        cookie,
    )
    .await;

    let response = app
        .post_form(
            &project_task_url(&project_path, &task_path, "drop"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location_of(&response), project_path);

    let task_page = body_text(app.get(&task_path, cookie).await).await;
    assert!(task_page.contains("below the horizon"));
    assert!(
        task_page.contains("bounced 1x"),
        "dropping from a non-Done column without finishing must count as a bounce: {task_page}"
    );
}

#[tokio::test]
async fn dropping_via_hx_returns_both_oob_lists_with_the_task_moved_back() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (project_path, _columns) = setup_project(&app).await;
    let task_path = new_task_in(&app, &project_path, "Regrout the shower").await;
    app.post_form(
        &project_task_url(&project_path, &task_path, "raise"),
        &[("csrf_token", support::DEV_CSRF_TOKEN)],
        cookie,
    )
    .await;

    let response = app
        .post_form_hx(
            &project_task_url(&project_path, &task_path, "drop"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(body.contains("hx-swap-oob=\"true\""));
    assert!(body.contains("id=\"on-board-list\""));
    assert!(body.contains("id=\"below-list\""));
    assert!(body.contains("Regrout the shower"));
}

#[tokio::test]
async fn dropping_from_the_done_column_does_not_count_as_a_bounce() {
    // `left_a_done_column` (`crate::handlers::tasks::drop_task_with_bounce_accounting`,
    // shared by both the global and the project-scoped drop endpoints) is
    // `true` here, which `bounce_to_below` (`docs/DOMAIN.md` §5) treats as
    // "finished, not given up on" -- the opposite of the plain drop above.
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (project_path, columns) = setup_project(&app).await;
    let task_path = new_task_in(&app, &project_path, "Regrout the shower").await;
    let done_column = columns[2];

    // Reach the Done column via the *global* raise endpoint -- the
    // project-scoped one always raises onto the entry (To-Do) column, and
    // only the global endpoint takes an explicit `column_id`.
    app.post_form(
        &format!("{task_path}/raise"),
        &[
            ("csrf_token", support::DEV_CSRF_TOKEN),
            ("column_id", &done_column.as_uuid().to_string()),
        ],
        cookie,
    )
    .await;

    let response = app
        .post_form(
            &project_task_url(&project_path, &task_path, "drop"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let task_page = body_text(app.get(&task_path, cookie).await).await;
    assert!(task_page.contains("below the horizon"));
    assert!(
        !task_page.contains("bounced"),
        "leaving a Done column must not increment the bounce count: {task_page}"
    );
}

#[tokio::test]
async fn raising_with_a_mismatched_csrf_token_is_rejected() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (project_path, _columns) = setup_project(&app).await;
    let task_path = new_task_in(&app, &project_path, "Regrout the shower").await;

    let response = app
        .post_form(
            &project_task_url(&project_path, &task_path, "raise"),
            &[("csrf_token", "wrong")],
            cookie,
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let board = app.store.columns_with_items().await.unwrap();
    assert!(
        board.iter().all(|c| c.items.is_empty()),
        "a rejected CSRF token must not raise the task"
    );
}

#[tokio::test]
async fn dropping_with_a_mismatched_csrf_token_is_rejected() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (project_path, _columns) = setup_project(&app).await;
    let task_path = new_task_in(&app, &project_path, "Regrout the shower").await;
    app.post_form(
        &project_task_url(&project_path, &task_path, "raise"),
        &[("csrf_token", support::DEV_CSRF_TOKEN)],
        cookie,
    )
    .await;

    let response = app
        .post_form(
            &project_task_url(&project_path, &task_path, "drop"),
            &[("csrf_token", "wrong")],
            cookie,
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let task_page = body_text(app.get(&task_path, cookie).await).await;
    assert!(
        task_page.contains("on the board"),
        "a rejected CSRF token must not drop the task: {task_page}"
    );
}

#[tokio::test]
async fn raising_into_a_full_entry_column_reverts_silently_over_hx_but_errors_on_a_plain_post() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (project_path, columns) = setup_project(&app).await;

    // Fill the entry (To-Do) column to its WIP limit via the *global* raise
    // endpoint, exactly like `tests/suggestion.rs` does -- the project-scoped
    // endpoint always raises onto the same entry column, so this has the
    // same effect and avoids needing this loop to look up the column id.
    for n in 0..DEFAULT_TODO_WIP_LIMIT {
        let filler_path = new_task_in(&app, &project_path, &format!("filler {n}")).await;
        let raise = app
            .post_form(
                &format!("{filler_path}/raise"),
                &[
                    ("csrf_token", support::DEV_CSRF_TOKEN),
                    ("column_id", &columns[0].as_uuid().to_string()),
                ],
                cookie,
            )
            .await;
        assert_eq!(raise.status(), StatusCode::SEE_OTHER);
    }

    let task_path = new_task_in(&app, &project_path, "One too many").await;

    // Plain form: 422 carrying the WIP-limit message, task stays below.
    let non_hx = app
        .post_form(
            &project_task_url(&project_path, &task_path, "raise"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(non_hx.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let non_hx_body = body_text(non_hx).await;
    assert!(non_hx_body.contains("work-in-progress limit"));

    let task_page = body_text(app.get(&task_path, cookie).await).await;
    assert!(
        task_page.contains("below the horizon"),
        "a WIP-limit rejection must leave the task where it was: {task_page}"
    );

    // HX: silently reverts -- 200 with the unchanged lists, no error text.
    let hx = app
        .post_form_hx(
            &project_task_url(&project_path, &task_path, "raise"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(hx.status(), StatusCode::OK);
    let hx_body = body_text(hx).await;
    assert!(hx_body.contains("id=\"below-list\""));
    assert!(
        !hx_body.contains("work-in-progress"),
        "the hx path has no per-card spot to show the WIP error: {hx_body}"
    );
}
