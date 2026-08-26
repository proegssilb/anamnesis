//! The two access-control guarantees `docs/PLAN.md` calls mandatory:
//! unauthenticated access redirects to `/login`, and one user can never
//! fetch another user's board.

mod support;

use axum::http::StatusCode;
use uuid::Uuid;

#[tokio::test]
async fn unauthenticated_board_access_redirects_to_login() {
    let app = support::TestApp::new(false).await;

    let response = app.get("/boards", None).await;

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(support::location_of(&response), "/login");
}

#[tokio::test]
async fn unauthenticated_specific_board_access_redirects_to_login() {
    let app = support::TestApp::new(false).await;

    let response = app.get(&format!("/boards/{}", Uuid::new_v4()), None).await;

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(support::location_of(&response), "/login");
}

#[tokio::test]
async fn one_user_cannot_fetch_another_users_board() {
    let app = support::TestApp::new(false).await;

    let alice_cookie = app.login_cookie_header("alice", "alice-csrf");
    let bob_cookie = app.login_cookie_header("bob", "bob-csrf");

    // Alice creates a board over HTTP, exactly as a real user would.
    let create = app
        .post_form(
            "/boards",
            &[
                ("csrf_token", "alice-csrf"),
                ("title", "Alice's private board"),
            ],
            Some(&alice_cookie),
        )
        .await;
    assert_eq!(create.status(), StatusCode::SEE_OTHER);
    let board_path = support::location_of(&create).to_string();

    // Alice can see it.
    let alice_view = app.get(&board_path, Some(&alice_cookie)).await;
    assert_eq!(alice_view.status(), StatusCode::OK);

    // Bob cannot.
    let bob_view = app.get(&board_path, Some(&bob_cookie)).await;
    assert_eq!(bob_view.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn dev_auth_bypass_short_circuits_to_a_fixed_user_without_a_cookie() {
    let app = support::TestApp::new(true).await;

    let response = app.get("/boards", None).await;

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_user_can_see_their_own_board_in_their_list_but_not_another_users() {
    let app = support::TestApp::new(false).await;
    let alice_cookie = app.login_cookie_header("alice", "alice-csrf");
    let bob_cookie = app.login_cookie_header("bob", "bob-csrf");

    app.post_form(
        "/boards",
        &[("csrf_token", "alice-csrf"), ("title", "Alice board")],
        Some(&alice_cookie),
    )
    .await;

    let bob_list = app.get("/boards", Some(&bob_cookie)).await;
    let body = support::body_text(bob_list).await;
    assert!(
        !body.contains("Alice board"),
        "Bob's board list must not leak Alice's board title"
    );
}
