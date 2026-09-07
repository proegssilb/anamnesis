//! `tower::ServiceExt::oneshot` coverage for issue #44: an `@[Name](user:ID)`
//! mention token (`crate::handlers::markdown::render`) renders highlighted
//! only for the user it names — "do nothing" for everyone else, per the
//! issue's own wording.

mod support;

use axum::http::StatusCode;

use support::{TestApp, body_text};

async fn create_area(app: &TestApp, cookie: &str, csrf: &str, title: &str) -> String {
    support::new_area(app, title, csrf, Some(cookie)).await
}

async fn create_project(
    app: &TestApp,
    cookie: &str,
    csrf: &str,
    area_path: &str,
    title: &str,
) -> String {
    support::new_project_in(app, area_path, title, csrf, Some(cookie)).await
}

#[tokio::test]
async fn a_task_description_mentioning_the_viewer_renders_highlighted_but_plain_for_anyone_else() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = create_area(&app, &admin_cookie, "admin-token", "Home").await;
    let project_path =
        create_project(&app, &admin_cookie, "admin-token", &area_path, "Repaint").await;
    let bob_cookie = app.login_cookie_header("bob", "bob-token");
    app.post_form(
        &format!("{project_path}/members"),
        &[
            ("csrf_token", "admin-token"),
            ("user_id", "bob"),
            ("role", "member"),
        ],
        Some(&admin_cookie),
    )
    .await;

    let create_task = app
        .post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", "admin-token"),
                ("title", "Buy paint"),
                (
                    "description",
                    "@[Bob](user:bob), can you grab the blue paint?",
                ),
            ],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(create_task.status(), StatusCode::SEE_OTHER);
    let task_path = support::location_of(&create_task).to_string();

    let bob_view = body_text(app.get(&task_path, Some(&bob_cookie)).await).await;
    assert!(
        bob_view.contains("<strong>@Bob</strong>"),
        "bob should see himself highlighted"
    );

    let admin_view = body_text(app.get(&task_path, Some(&admin_cookie)).await).await;
    assert!(
        !admin_view.contains("<strong>@Bob</strong>"),
        "admin viewing a mention of someone else should see it, but not highlighted"
    );
    assert!(admin_view.contains("@Bob"));
}

#[tokio::test]
async fn a_comment_mentioning_the_viewer_renders_highlighted() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = create_area(&app, &admin_cookie, "admin-token", "Home").await;
    let project_path =
        create_project(&app, &admin_cookie, "admin-token", &area_path, "Repaint").await;

    let create_task = app
        .post_form(
            &format!("{project_path}/tasks"),
            &[("csrf_token", "admin-token"), ("title", "Buy paint")],
            Some(&admin_cookie),
        )
        .await;
    let task_path = support::location_of(&create_task).to_string();

    let comment = app
        .post_form(
            &format!("{task_path}/comments"),
            &[
                ("csrf_token", "admin-token"),
                (
                    "body",
                    "@[Admin](user:admin) please review <script>x</script>",
                ),
            ],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(comment.status(), StatusCode::SEE_OTHER);

    let view = body_text(app.get(&task_path, Some(&admin_cookie)).await).await;
    assert!(view.contains("<strong>@Admin</strong>"));
    assert!(!view.contains("<script>x</script>"));
}
