//! `tower::ServiceExt::oneshot` coverage for issue #43: Area/Project/Task
//! descriptions strip HTML on input and render as Markdown on output
//! (`crate::handlers::markdown`).

mod support;

use support::{DEV_CSRF_TOKEN, TestApp, body_text, new_active_project, new_task_as};

#[tokio::test]
async fn a_tasks_description_is_rendered_as_markdown_with_html_stripped_at_save_time() {
    let app = TestApp::new(true).await;
    let (_, project_path) = new_active_project(&app, "Home", "Kitchen remodel", None).await;
    let task_path = new_task_as(&app, &project_path, "Tile the floor", DEV_CSRF_TOKEN, None).await;

    let response = app
        .post_form(
            &format!("{task_path}/description"),
            &[
                ("csrf_token", DEV_CSRF_TOKEN),
                (
                    "description",
                    "# Plan\n\n**bold** and <script>alert(1)</script> more",
                ),
            ],
            None,
        )
        .await;
    let body = body_text(response).await;

    // Real markdown, rendered to real HTML.
    assert!(body.contains("<h1>Plan</h1>"));
    assert!(body.contains("<strong>bold</strong>"));
    // The injected script tag and its content never survive strip_html at
    // save time (the page's own `<script src="...">` includes are still
    // present, so this checks for the *inline* tag specifically).
    assert!(!body.contains("<script>"));
    assert!(!body.contains("alert(1)"));

    // The raw markdown source is still what comes back in the edit
    // textarea, not the rendered HTML.
    assert!(body.contains("# Plan"));
}

#[tokio::test]
async fn a_project_description_with_a_link_gets_safe_rel_attributes() {
    let app = TestApp::new(true).await;
    let (_, project_path) = new_active_project(&app, "Home", "Kitchen remodel", None).await;

    let response = app
        .post_form(
            &format!("{project_path}/description"),
            &[
                ("csrf_token", DEV_CSRF_TOKEN),
                ("description", "See [the plan](https://example.com/plan)"),
            ],
            None,
        )
        .await;
    let body = body_text(response).await;

    assert!(body.contains(r#"href="https://example.com/plan""#));
    assert!(body.contains("noopener"));
}
