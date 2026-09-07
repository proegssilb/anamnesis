//! `tower::ServiceExt::oneshot` coverage for the `blocked_by` relationship
//! kind (issue #48): the "Add relationship" form only ever let a task say it
//! `blocks` another, never that it is itself `blocked_by` one — an
//! asymmetry, since `RelationshipKind::reverse_label` for `blocks` has
//! always been "blocked by". `blocked_by` has no built-in id of its own; the
//! route stores it as a `blocks` edge with `from`/`to` swapped.

mod support;

use axum::http::StatusCode;

use anamnesis_app::RelationshipRepository;
use support::{DEV_CSRF_TOKEN, TestApp, body_text};

#[tokio::test]
async fn marking_a_task_blocked_by_another_stores_the_edge_reversed() {
    let app = TestApp::new(true).await;
    let (_, project_path) =
        support::new_area_with_project(&app, "Home", "Kitchen remodel", DEV_CSRF_TOKEN, None).await;
    support::set_project_status(&app, &project_path, "active", DEV_CSRF_TOKEN, None).await;

    let task_a_path =
        support::new_task_as(&app, &project_path, "Tile the floor", DEV_CSRF_TOKEN, None).await;
    let task_b_path =
        support::new_task_as(&app, &project_path, "Order the tile", DEV_CSRF_TOKEN, None).await;
    let task_b_id_str = task_b_path.trim_start_matches("/tasks/").to_string();

    // From task A's page, declare A is blocked_by B.
    let create = app
        .post_form(
            &format!("{task_a_path}/relationships"),
            &[
                ("csrf_token", DEV_CSRF_TOKEN),
                ("to_task_id", task_b_id_str.as_str()),
                ("kind", "blocked_by"),
            ],
            None,
        )
        .await;
    assert_eq!(create.status(), StatusCode::SEE_OTHER);

    let task_a_id: uuid::Uuid = task_a_path.trim_start_matches("/tasks/").parse().unwrap();
    let task_b_id: uuid::Uuid = task_b_id_str.parse().unwrap();
    let all = app
        .store
        .list_for_task(anamnesis_core::TaskId::new(task_a_id))
        .await
        .unwrap();
    assert_eq!(all.len(), 1, "exactly one relationship must exist");
    // Stored as a `blocks` edge from B to A, not A to B.
    assert_eq!(all[0].from_task_id, anamnesis_core::TaskId::new(task_b_id));
    assert_eq!(all[0].to_task_id, anamnesis_core::TaskId::new(task_a_id));
    assert_eq!(all[0].kind_id, anamnesis_core::builtin_blocks().id);

    let a_page = body_text(app.get(&task_a_path, None).await).await;
    assert!(
        a_page.contains("blocked by") && a_page.contains("Order the tile"),
        "task A's page should read as blocked by task B"
    );

    let b_page = body_text(app.get(&task_b_path, None).await).await;
    assert!(
        b_page.contains("blocks <a") && b_page.contains("Tile the floor"),
        "task B's page should read as blocking task A"
    );
}
