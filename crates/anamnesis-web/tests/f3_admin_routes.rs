//! `tower::ServiceExt::oneshot` coverage for Phase F3: the seven use cases
//! that were fully implemented in `anamnesis-app` but unreachable over HTTP
//! before this phase — `edit_area`, `archive_task`/`unarchive_task`,
//! `archive_project`/`unarchive_project`, and `add_field_definition` here;
//! `add_file_attachment` gets its own file (`tests/f3_attachments.rs`), and
//! the archived-search round trip its own (`tests/search.rs`).

mod support;

use axum::http::StatusCode;

use anamnesis_app::BoardQuery;
use support::{TestApp, body_text, location_of};

async fn create_area(app: &TestApp, title: &str) -> String {
    location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", title),
                ("description", ""),
            ],
            None,
        )
        .await,
    )
    .to_string()
}

async fn create_project(app: &TestApp, area_path: &str, title: &str) -> String {
    location_of(
        &app.post_form(
            &format!("{area_path}/projects"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", title),
                ("description", ""),
            ],
            None,
        )
        .await,
    )
    .to_string()
}

async fn create_task(app: &TestApp, project_path: &str, title: &str) -> String {
    location_of(
        &app.post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", title),
                ("description", ""),
            ],
            None,
        )
        .await,
    )
    .to_string()
}

// --- edit_area ---

#[tokio::test]
async fn edit_area_happy_path_updates_title_and_description() {
    let app = TestApp::new(true).await;
    let area_path = create_area(&app, "Homesteading").await;

    let response = app
        .post_form(
            &area_path,
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Homesteading, revised"),
                ("description", "now with a description"),
            ],
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location_of(&response), area_path);

    let body = body_text(app.get(&area_path, None).await).await;
    assert!(body.contains("Homesteading, revised"));
    assert!(body.contains("now with a description"));
}

#[tokio::test]
async fn edit_area_without_a_valid_csrf_token_is_rejected() {
    let app = TestApp::new(true).await;
    let area_path = create_area(&app, "Homesteading").await;

    let response = app
        .post_form(
            &area_path,
            &[
                ("csrf_token", "not-the-real-token"),
                ("title", "Sneaky rename"),
                ("description", ""),
            ],
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let body = body_text(app.get(&area_path, None).await).await;
    assert!(!body.contains("Sneaky rename"));
}

#[tokio::test]
async fn edit_area_by_an_ungranted_user_is_forbidden() {
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

    let stranger_cookie = app.login_cookie_header("stranger", "stranger-token");
    let response = app
        .post_form(
            &area_path,
            &[
                ("csrf_token", "stranger-token"),
                ("title", "Hijacked"),
                ("description", ""),
            ],
            Some(&stranger_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// --- archive_task / unarchive_task ---

#[tokio::test]
async fn archive_then_unarchive_task_round_trips() {
    let app = TestApp::new(true).await;
    let area_path = create_area(&app, "Home hunting").await;
    let project_path = create_project(&app, &area_path, "House shopping").await;
    let task_path = create_task(&app, &project_path, "123 Maple St").await;

    // Archiving vanishes it from the project's flat list.
    let archive = app
        .post_form(
            &format!("{task_path}/archive"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            None,
        )
        .await;
    assert_eq!(archive.status(), StatusCode::SEE_OTHER);
    assert_eq!(location_of(&archive), task_path);

    let project_body = body_text(app.get(&project_path, None).await).await;
    assert!(
        !project_body.contains("123 Maple St"),
        "an archived task must vanish from its project's flat list: {project_body}"
    );

    let task_body = body_text(app.get(&task_path, None).await).await;
    assert!(task_body.contains("archived"));

    // Unarchiving restores it.
    let unarchive = app
        .post_form(
            &format!("{task_path}/unarchive"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            None,
        )
        .await;
    assert_eq!(unarchive.status(), StatusCode::SEE_OTHER);

    let project_body = body_text(app.get(&project_path, None).await).await;
    assert!(
        project_body.contains("123 Maple St"),
        "unarchiving must restore the task to its project's flat list: {project_body}"
    );
}

#[tokio::test]
async fn archive_task_without_a_valid_csrf_token_is_rejected() {
    let app = TestApp::new(true).await;
    let area_path = create_area(&app, "Home hunting").await;
    let project_path = create_project(&app, &area_path, "House shopping").await;
    let task_path = create_task(&app, &project_path, "123 Maple St").await;

    let response = app
        .post_form(
            &format!("{task_path}/archive"),
            &[("csrf_token", "wrong-token")],
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn archive_task_by_an_ungranted_user_is_forbidden() {
    let (app, task_path, stranger_cookie) = support::setup_task_as_admin().await;

    let response = app
        .post_form(
            &format!("{task_path}/archive"),
            &[("csrf_token", "stranger-token")],
            Some(&stranger_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// --- archive_project / unarchive_project ---

#[tokio::test]
async fn archive_then_unarchive_project_round_trips() {
    let app = TestApp::new(true).await;
    let area_path = create_area(&app, "Home hunting").await;
    let project_path = create_project(&app, &area_path, "House shopping").await;

    let archive = app
        .post_form(
            &format!("{project_path}/archive"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            None,
        )
        .await;
    assert_eq!(archive.status(), StatusCode::SEE_OTHER);

    let area_body = body_text(app.get(&area_path, None).await).await;
    assert!(
        !area_body.contains("House shopping"),
        "an archived project must vanish from its area's project board: {area_body}"
    );

    let unarchive = app
        .post_form(
            &format!("{project_path}/unarchive"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            None,
        )
        .await;
    assert_eq!(unarchive.status(), StatusCode::SEE_OTHER);

    let area_body = body_text(app.get(&area_path, None).await).await;
    assert!(
        area_body.contains("House shopping"),
        "unarchiving must restore the project to its area's project board: {area_body}"
    );
}

#[tokio::test]
async fn archive_project_without_a_valid_csrf_token_is_rejected() {
    let app = TestApp::new(true).await;
    let area_path = create_area(&app, "Home hunting").await;
    let project_path = create_project(&app, &area_path, "House shopping").await;

    let response = app
        .post_form(
            &format!("{project_path}/archive"),
            &[("csrf_token", "wrong-token")],
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn archive_project_by_an_ungranted_user_is_forbidden() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", "admin-token"),
                ("title", "Home hunting"),
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
                ("title", "House shopping"),
                ("description", ""),
            ],
            Some(&admin_cookie),
        )
        .await,
    )
    .to_string();

    let stranger_cookie = app.login_cookie_header("stranger", "stranger-token");
    let response = app
        .post_form(
            &format!("{project_path}/archive"),
            &[("csrf_token", "stranger-token")],
            Some(&stranger_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// --- add_field_definition: the house-hunting end-to-end path ---

#[tokio::test]
async fn defining_a_field_setting_it_and_seeing_it_on_the_card_needs_no_sql_seeding() {
    let app = TestApp::new(true).await;
    let area_path = create_area(&app, "Home hunting").await;
    let project_path = create_project(&app, &area_path, "House shopping").await;
    let task_path = create_task(&app, &project_path, "123 Maple St").await;

    define_currency_field_shown_on_card(&app, &project_path).await;
    set_currency_value_on_task(&app, &task_path).await;
    raise_and_assert_value_shows_on_card(&app, &task_path, "123 Maple St").await;
}

/// Defines a Currency field through the UI (no SQL, no direct app-layer
/// call), with `show_on_card` checked, and confirms it renders on the
/// project page.
async fn define_currency_field_shown_on_card(app: &TestApp, project_path: &str) {
    let define = app
        .post_form(
            &format!("{project_path}/fields"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("name", "Price"),
                ("kind", "currency"),
                ("show_on_card", "1"),
            ],
            None,
        )
        .await;
    assert_eq!(define.status(), StatusCode::SEE_OTHER);
    assert_eq!(location_of(&define), project_path);

    let project_body = body_text(app.get(project_path, None).await).await;
    assert!(project_body.contains("Price"));
    assert!(project_body.contains("currency"));
    assert!(project_body.contains("shown on card"));
}

/// Scrapes the field's id from the task page's rendered field form
/// (`action="/tasks/{id}/fields/{field_id}"`) and sets a value on it --
/// proof the field is reachable without seeding it through SQL or the app
/// layer directly.
async fn set_currency_value_on_task(app: &TestApp, task_path: &str) {
    let task_body = body_text(app.get(task_path, None).await).await;
    let fields_marker = "/fields/";
    let start = task_body
        .find(fields_marker)
        .expect("the field-set form action must be present");
    let after = &task_body[start + fields_marker.len()..];
    let field_id: String = after.chars().take_while(|c| *c != '"').collect();
    assert!(
        !field_id.is_empty(),
        "could not find the field id: {task_body}"
    );

    let set_value = app
        .post_form(
            &format!("{task_path}/fields/{field_id}"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("value", "419999.99"),
                ("currency", "usd"),
            ],
            None,
        )
        .await;
    assert_eq!(set_value.status(), StatusCode::SEE_OTHER);
}

/// Raises the task onto the board's entry column and confirms the
/// show_on_card field's value renders on its card. The entry column is the
/// board's first (lowest-position) column -- always To-Do per
/// `crate::bootstrap` -- read directly via `BoardQuery` rather than
/// scraping the board page (that column has no cards in it yet, so its
/// reposition-form picker is not even rendered).
async fn raise_and_assert_value_shows_on_card(app: &TestApp, task_path: &str, title: &str) {
    let columns = app.store.columns_with_items().await.unwrap();
    let column_id = columns
        .first()
        .expect("bootstrap always seeds at least one board column")
        .column
        .id
        .to_string();

    let raise = app
        .post_form(
            &format!("{task_path}/raise"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("column_id", &column_id),
            ],
            None,
        )
        .await;
    assert_eq!(raise.status(), StatusCode::SEE_OTHER);

    let board_body = body_text(app.get("/board", None).await).await;
    assert!(
        board_body.contains(title),
        "the task must appear on the board: {board_body}"
    );
    assert!(
        board_body.contains("419,999.99") || board_body.contains("419999.99"),
        "the show_on_card Price field must render on the board card: {board_body}"
    );
}

#[tokio::test]
async fn add_field_definition_without_a_valid_csrf_token_is_rejected() {
    let app = TestApp::new(true).await;
    let area_path = create_area(&app, "Home hunting").await;
    let project_path = create_project(&app, &area_path, "House shopping").await;

    let response = app
        .post_form(
            &format!("{project_path}/fields"),
            &[
                ("csrf_token", "wrong-token"),
                ("name", "Price"),
                ("kind", "currency"),
            ],
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn add_field_definition_by_a_non_admin_is_forbidden() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", "admin-token"),
                ("title", "Home hunting"),
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
                ("title", "House shopping"),
                ("description", ""),
            ],
            Some(&admin_cookie),
        )
        .await,
    )
    .to_string();

    // A stranger with no grant at all is refused.
    let stranger_cookie = app.login_cookie_header("stranger", "stranger-token");
    let response = app
        .post_form(
            &format!("{project_path}/fields"),
            &[
                ("csrf_token", "stranger-token"),
                ("name", "Price"),
                ("kind", "currency"),
            ],
            Some(&stranger_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
