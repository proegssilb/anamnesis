//! `tower::ServiceExt::oneshot` coverage for issue #35: a board card's title
//! rendered as a plain link with no visual weight, and gave no indication at
//! all of which project (or area) the task belonged to — a problem the
//! moment two projects share the board. `crate::handlers::board::task_card_view`
//! now resolves and renders both.

mod support;

use anamnesis_app::BoardQuery;

use support::{TestApp, body_text, new_area, new_project_in, new_task};

#[tokio::test]
async fn a_board_card_names_its_task_project_and_area() {
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
    let task_path = new_task(&app, &project_path, "Buy primer", cookie).await;

    let todo = app.store.columns_with_items().await.unwrap()[0].column.id;
    app.post_form(
        &format!("{task_path}/raise"),
        &[
            ("csrf_token", support::DEV_CSRF_TOKEN),
            ("column_id", &todo.to_string()),
        ],
        cookie,
    )
    .await;

    let board = body_text(app.get("/board", cookie).await).await;
    assert!(
        board.contains(r#"<h4 class="card-title">"#),
        "the card title must render as a small heading: {board}"
    );
    assert!(
        board.contains("Buy primer"),
        "the task's own title must still appear: {board}"
    );
    assert!(
        board.contains("Repaint the shed"),
        "the card must name its project: {board}"
    );
    assert!(
        board.contains("Home Ops"),
        "the card must name its area: {board}"
    );
}
