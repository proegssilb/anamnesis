//! `tower::ServiceExt::oneshot` coverage for `POST /board/reposition`
//! (`docs/DOMAIN.md` §8: "Sortable drags, htmx persists"): the same
//! endpoint reached two ways — an htmx drag (`HX-Request: true`, exercised
//! here the same way `static/app.js`'s `htmx.ajax` call would) and the
//! plain-form fallback (`templates/_reposition_form.html`) — must both
//! actually reorder the column, and progressive enhancement means neither
//! path is allowed to be the only one that works.

mod support;

use axum::http::StatusCode;

use anamnesis_app::{BoardItem, BoardQuery};
use support::TestApp;

/// Creates an area, an active project, and returns its path plus the
/// board's first ("To-Do") column id.
async fn setup_project(app: &TestApp) -> (String, anamnesis_core::ColumnId) {
    let cookie: Option<&str> = None;
    let area_path = support::location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Home"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    let project_path = support::location_of(
        &app.post_form(
            &format!("{area_path}/projects"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "House hunting"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    let todo_column = app.store.columns_with_items().await.unwrap()[0].column.id;
    (project_path, todo_column)
}

/// Creates a task under `project_path` and raises it onto `column` —
/// returns its id.
async fn create_and_raise(
    app: &TestApp,
    project_path: &str,
    title: &str,
    column: uuid::Uuid,
) -> uuid::Uuid {
    let cookie: Option<&str> = None;
    let task_path = support::location_of(
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
    .to_string();
    let task_id: uuid::Uuid = task_path.trim_start_matches("/tasks/").parse().unwrap();
    app.post_form(
        &format!("{task_path}/raise"),
        &[
            ("csrf_token", support::DEV_CSRF_TOKEN),
            ("column_id", &column.to_string()),
        ],
        cookie,
    )
    .await;
    task_id
}

fn task_order(items: &[BoardItem]) -> Vec<uuid::Uuid> {
    items
        .iter()
        .map(|item| match item {
            BoardItem::Task(t) => t.id.as_uuid(),
            BoardItem::Tangle(t) => t.id.as_uuid(),
        })
        .collect()
}

#[tokio::test]
async fn a_drag_driven_reposition_via_the_htmx_endpoint_reorders_the_column() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (project_path, todo) = setup_project(&app).await;

    let house_a = create_and_raise(&app, &project_path, "123 Maple St", todo.as_uuid()).await;
    let house_b = create_and_raise(&app, &project_path, "456 Oak Ave", todo.as_uuid()).await;

    // Starting order is creation order: A, then B.
    let before = app.store.columns_with_items().await.unwrap();
    let before_todo = before.iter().find(|c| c.column.id == todo).unwrap();
    assert_eq!(task_order(&before_todo.items), vec![house_a, house_b]);

    // Drag B to the front — exactly what `static/app.js`'s Sortable
    // `onEnd` handler sends via `htmx.ajax`: an `HX-Request` POST naming
    // the dragged item, its destination column, and its new index.
    let response = app
        .post_form_hx(
            "/board/reposition",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("item_kind", "task"),
                ("item_id", &house_b.to_string()),
                ("column_id", &todo.to_string()),
                ("position", "0"),
            ],
            cookie,
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "an HX-Request reposition returns the fragment directly, not a redirect"
    );
    let body = support::body_text(response).await;
    assert!(
        body.contains("hx-swap-oob=\"true\""),
        "the response must be an out-of-band column fragment: {body}"
    );
    assert!(body.contains(&format!("id=\"column-{todo}\"")));

    let after = app.store.columns_with_items().await.unwrap();
    let after_todo = after.iter().find(|c| c.column.id == todo).unwrap();
    assert_eq!(
        task_order(&after_todo.items),
        vec![house_b, house_a],
        "the dragged card must now come first"
    );
}

#[tokio::test]
async fn the_same_reposition_via_the_plain_form_fallback_also_reorders_the_column() {
    // Progressive enhancement (`docs/DOMAIN.md` §8): the exact same move,
    // submitted the way a no-JS browser's plain `<form>` would (no
    // `HX-Request` header), must produce the identical result.
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (project_path, todo) = setup_project(&app).await;

    let house_a = create_and_raise(&app, &project_path, "123 Maple St", todo.as_uuid()).await;
    let house_b = create_and_raise(&app, &project_path, "456 Oak Ave", todo.as_uuid()).await;

    let response = app
        .post_form(
            "/board/reposition",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("item_kind", "task"),
                ("item_id", &house_b.to_string()),
                ("column_id", &todo.to_string()),
                ("position", "0"),
            ],
            cookie,
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "a plain form submit gets the usual redirect-after-POST, not a fragment"
    );
    assert_eq!(support::location_of(&response), "/board");

    let after = app.store.columns_with_items().await.unwrap();
    let after_todo = after.iter().find(|c| c.column.id == todo).unwrap();
    assert_eq!(
        task_order(&after_todo.items),
        vec![house_b, house_a],
        "the plain-form fallback must reorder the column exactly like the htmx path"
    );
}

#[tokio::test]
async fn repositioning_across_columns_renumbers_both() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (project_path, todo) = setup_project(&app).await;
    let doing = app.store.columns_with_items().await.unwrap()[1].column.id;

    let house_a = create_and_raise(&app, &project_path, "123 Maple St", todo.as_uuid()).await;
    let house_b = create_and_raise(&app, &project_path, "456 Oak Ave", todo.as_uuid()).await;

    let response = app
        .post_form_hx(
            "/board/reposition",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("item_kind", "task"),
                ("item_id", &house_a.to_string()),
                ("column_id", &doing.to_string()),
                ("position", "0"),
            ],
            cookie,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = support::body_text(response).await;
    // Both the destination and the vacated source column must come back,
    // each independently addressed.
    assert!(body.contains(&format!("id=\"column-{doing}\"")));
    assert!(body.contains(&format!("id=\"column-{todo}\"")));

    let after = app.store.columns_with_items().await.unwrap();
    let after_todo = after.iter().find(|c| c.column.id == todo).unwrap();
    let after_doing = after.iter().find(|c| c.column.id == doing).unwrap();
    assert_eq!(
        task_order(&after_todo.items),
        vec![house_b],
        "the source column must close the gap left behind"
    );
    assert_eq!(task_order(&after_doing.items), vec![house_a]);
}
