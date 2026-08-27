//! `tower::ServiceExt::oneshot` coverage for the area -> project -> task
//! chain rendering, and for raising a task onto the global task board and
//! dropping it back below the horizon (`docs/DOMAIN.md` §2).

mod support;

use axum::http::StatusCode;

use anamnesis_app::BoardQuery;
use support::{TestApp, body_text, location_of};

#[tokio::test]
async fn area_then_project_then_task_all_render() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None; // dev-auth-bypass needs no cookie at all.

    let create_area = app
        .post_form(
            "/areas",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Home"),
                ("description", "household stuff"),
            ],
            cookie,
        )
        .await;
    assert_eq!(create_area.status(), StatusCode::SEE_OTHER);
    let area_path = location_of(&create_area).to_string();

    let area_page = app.get(&area_path, cookie).await;
    assert_eq!(area_page.status(), StatusCode::OK);
    let area_body = body_text(area_page).await;
    assert!(area_body.contains("Home"));

    let create_project = app
        .post_form(
            &format!("{area_path}/projects"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Kitchen remodel"),
                ("description", "regrout, repaint"),
            ],
            cookie,
        )
        .await;
    assert_eq!(create_project.status(), StatusCode::SEE_OTHER);
    let project_path = location_of(&create_project).to_string();

    let project_page = app.get(&project_path, cookie).await;
    assert_eq!(project_page.status(), StatusCode::OK);
    let project_body = body_text(project_page).await;
    assert!(project_body.contains("Kitchen remodel"));

    let create_task = app
        .post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Regrout the shower"),
                ("description", ""),
            ],
            cookie,
        )
        .await;
    assert_eq!(create_task.status(), StatusCode::SEE_OTHER);
    let task_path = location_of(&create_task).to_string();

    let task_page = app.get(&task_path, cookie).await;
    assert_eq!(task_page.status(), StatusCode::OK);
    let task_body = body_text(task_page).await;
    assert!(task_body.contains("Regrout the shower"));
    assert!(
        task_body.contains("below the horizon"),
        "a freshly created task starts below the horizon"
    );

    // It also shows up in the project's flat task list, below the horizon.
    let project_page_again = app.get(&project_path, cookie).await;
    let project_body_again = body_text(project_page_again).await;
    assert!(project_body_again.contains("Regrout the shower"));
}

#[tokio::test]
async fn raising_a_task_onto_the_board_and_dropping_it_back() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let area_path = location_of(
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
    let project_path = location_of(
        &app.post_form(
            &format!("{area_path}/projects"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Kitchen remodel"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    let task_path = location_of(
        &app.post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Regrout the shower"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();

    let todo_column = app.store.columns_with_items().await.unwrap()[0].column.id;

    let raise = app
        .post_form(
            &format!("{task_path}/raise"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("column_id", &todo_column.to_string()),
            ],
            cookie,
        )
        .await;
    assert_eq!(raise.status(), StatusCode::SEE_OTHER);

    let after_raise = body_text(app.get(&task_path, cookie).await).await;
    assert!(
        after_raise.contains("on the board"),
        "task must now show as on the board"
    );

    let board_after_raise = body_text(app.get("/board", cookie).await).await;
    assert!(board_after_raise.contains("Regrout the shower"));

    let drop = app
        .post_form(
            &format!("{task_path}/drop"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(drop.status(), StatusCode::SEE_OTHER);

    let after_drop = body_text(app.get(&task_path, cookie).await).await;
    assert!(
        after_drop.contains("below the horizon"),
        "task must be back below the horizon after dropping"
    );
    assert!(
        after_drop.contains("bounced 1x"),
        "dropping without finishing must count as a bounce"
    );

    let board_after_drop = body_text(app.get("/board", cookie).await).await;
    assert!(!board_after_drop.contains("Regrout the shower"));
}

#[tokio::test]
async fn double_submitting_a_drop_only_bounces_once() {
    // Regression: a double-submitted form (or a browser re-POST on
    // refresh) sends the same `/tasks/{id}/drop` twice. The first POST is
    // a genuine `OnBoard -> Below` transition and must bounce; the second
    // POST hits a task that is already `Below` and must be a no-op.
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let area_path = location_of(
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
    let project_path = location_of(
        &app.post_form(
            &format!("{area_path}/projects"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Kitchen remodel"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    let task_path = location_of(
        &app.post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Regrout the shower"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();

    let todo_column = app.store.columns_with_items().await.unwrap()[0].column.id;

    app.post_form(
        &format!("{task_path}/raise"),
        &[
            ("csrf_token", support::DEV_CSRF_TOKEN),
            ("column_id", &todo_column.to_string()),
        ],
        cookie,
    )
    .await;

    let first_drop = app
        .post_form(
            &format!("{task_path}/drop"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(first_drop.status(), StatusCode::SEE_OTHER);

    let second_drop = app
        .post_form(
            &format!("{task_path}/drop"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(second_drop.status(), StatusCode::SEE_OTHER);

    let after_both_drops = body_text(app.get(&task_path, cookie).await).await;
    assert!(after_both_drops.contains("below the horizon"));
    assert!(
        after_both_drops.contains("bounced 1x"),
        "a double-submitted drop must not double the bounce count"
    );
    assert!(
        !after_both_drops.contains("bounced 2x"),
        "a double-submitted drop must not double the bounce count"
    );
}

#[tokio::test]
async fn moving_a_task_between_columns_does_not_bounce_it() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let area_path = location_of(
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
    let project_path = location_of(
        &app.post_form(
            &format!("{area_path}/projects"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Kitchen remodel"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    let task_path = location_of(
        &app.post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Regrout the shower"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();

    let columns = app.store.columns_with_items().await.unwrap();
    let todo = columns[0].column.id;
    let doing = columns[1].column.id;

    app.post_form(
        &format!("{task_path}/raise"),
        &[
            ("csrf_token", support::DEV_CSRF_TOKEN),
            ("column_id", &todo.to_string()),
        ],
        cookie,
    )
    .await;

    let move_response = app
        .post_form(
            &format!("{task_path}/raise"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("column_id", &doing.to_string()),
            ],
            cookie,
        )
        .await;
    assert_eq!(move_response.status(), StatusCode::SEE_OTHER);

    let after_move = body_text(app.get(&task_path, cookie).await).await;
    assert!(after_move.contains("on the board"));
    assert!(
        !after_move.contains("bounced"),
        "moving between board columns is not a bounce, only OnBoard -> Below is"
    );
}
