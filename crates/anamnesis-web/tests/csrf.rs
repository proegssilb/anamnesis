//! Every mutating form carries the session's CSRF token; a POST with a
//! missing or wrong one is rejected rather than applied.

mod support;

use axum::http::StatusCode;

#[tokio::test]
async fn post_without_a_valid_csrf_token_is_rejected() {
    let app = support::TestApp::new(true).await;

    let response = app
        .post_form(
            "/boards",
            &[
                ("csrf_token", "totally-wrong-token"),
                ("title", "Should not exist"),
            ],
            None,
        )
        .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // And it really did not create the board.
    let boards_page = app.get("/boards", None).await;
    let body = support::body_text(boards_page).await;
    assert!(!body.contains("Should not exist"));
}

#[tokio::test]
async fn post_with_a_missing_csrf_field_is_rejected_as_a_bad_request() {
    let app = support::TestApp::new(true).await;

    // No `csrf_token` field at all — the form itself is malformed, which
    // axum's `Form` extractor rejects before the handler ever runs.
    let response = app
        .post_form("/boards", &[("title", "No token field")], None)
        .await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn post_with_the_correct_csrf_token_succeeds() {
    let app = support::TestApp::new(true).await;

    let response = app
        .post_form(
            "/boards",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "A real board"),
            ],
            None,
        )
        .await;

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}
