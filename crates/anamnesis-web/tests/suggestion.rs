//! `tower::ServiceExt::oneshot` coverage for the suggestion prompt
//! (`docs/DOMAIN.md` §5): `Full` renders nothing at all, `Stuck` explains
//! itself, and an accepted offer actually raises the task.

mod support;

use axum::http::StatusCode;

use anamnesis_app::BoardQuery;
use anamnesis_web::bootstrap::DEFAULT_TODO_WIP_LIMIT;
use support::{TestApp, body_text, location_of};

#[tokio::test]
async fn a_fresh_empty_board_is_stuck_because_the_backlog_is_empty() {
    let app = TestApp::new(true).await;
    let body = body_text(app.get("/board", None).await).await;
    assert!(
        body.contains("suggestion-prompt"),
        "Stuck must render something, unlike Full"
    );
    assert!(body.contains("backlog is empty"));
}

#[tokio::test]
async fn a_full_board_renders_no_suggestion_prompt_at_all() {
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
    // Move the project to Active -- suggestion eligibility requires it.
    app.post_form(
        &format!("{project_path}/status"),
        &[
            ("csrf_token", support::DEV_CSRF_TOKEN),
            ("status", "active"),
        ],
        cookie,
    )
    .await;

    let todo_column = app.store.columns_with_items().await.unwrap()[0].column.id;

    // Fill the To-Do column to its WIP limit by raising that many tasks.
    for n in 0..DEFAULT_TODO_WIP_LIMIT {
        let task_path = location_of(
            &app.post_form(
                &format!("{project_path}/tasks"),
                &[
                    ("csrf_token", support::DEV_CSRF_TOKEN),
                    ("title", &format!("task {n}")),
                    ("description", ""),
                ],
                cookie,
            )
            .await,
        )
        .to_string();
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
    }

    let body = body_text(app.get("/board", cookie).await).await;
    assert!(
        !body.contains("suggestion-prompt"),
        "docs/DOMAIN.md §5: Full means silence -- no banner, no nudge, nothing rendered"
    );
    assert!(!body.contains("Worth picking up"));
    assert!(!body.contains("Nothing to suggest"));
}

#[tokio::test]
async fn accepting_an_offered_task_raises_it_onto_the_board() {
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
    app.post_form(
        &format!("{project_path}/status"),
        &[
            ("csrf_token", support::DEV_CSRF_TOKEN),
            ("status", "active"),
        ],
        cookie,
    )
    .await;
    app.post_form(
        &format!("{project_path}/tasks"),
        &[
            ("csrf_token", support::DEV_CSRF_TOKEN),
            ("title", "Regrout the shower"),
            ("description", ""),
        ],
        cookie,
    )
    .await;

    let board_body = body_text(app.get("/board", cookie).await).await;
    assert!(board_body.contains("Regrout the shower"));
    assert!(board_body.contains("suggestion-prompt"));

    let candidates = {
        use anamnesis_app::TaskRepository;
        // Only one task exists in this fresh test database -- find it via
        // the project's flat list rather than parsing HTML.
        app.store
            .list_by_project(anamnesis_core::ProjectId::new(
                uuid::Uuid::parse_str(project_path.trim_start_matches("/projects/")).unwrap(),
            ))
            .await
            .unwrap()
    };
    let task = candidates.first().expect("the task we just created");

    let accept = app
        .post_form(
            "/board/suggestion/accept",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("task_id", &task.id.to_string()),
            ],
            cookie,
        )
        .await;
    assert_eq!(accept.status(), StatusCode::SEE_OTHER);

    let after = body_text(app.get(&format!("/tasks/{}", task.id), cookie).await).await;
    assert!(after.contains("on the board"));
}
