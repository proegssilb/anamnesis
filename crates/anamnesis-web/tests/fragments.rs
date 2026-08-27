//! `tower::ServiceExt::oneshot` coverage for fragment-addressable rendering
//! (`docs/DOMAIN.md` §8): "one endpoint, two representations" rather than a
//! forked route table. `GET /board` is the primary example — a request
//! carrying `HX-Request: true` gets back just the columns fragment, anyone
//! else gets the full page.

mod support;

use axum::http::StatusCode;

use support::{TestApp, body_text};

#[tokio::test]
async fn a_plain_request_to_board_gets_the_full_page() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let response = app.get("/board", cookie).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(
        body.contains("<!doctype html>") || body.contains("<html"),
        "a plain GET must render the full page shell: {body}"
    );
    assert!(body.contains("Task board"), "the full page has its header");
    assert!(
        body.contains("id=\"board-columns\""),
        "the full page still contains the columns fragment inside it"
    );
}

#[tokio::test]
async fn an_hx_request_to_board_gets_only_the_columns_fragment() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let response = app.get_hx("/board", cookie).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(
        !body.contains("<!doctype html>") && !body.contains("<html"),
        "an HX-Request must get a bare fragment, not the page shell: {body}"
    );
    assert!(
        !body.contains("Task board"),
        "the fragment must not repeat the page's own header: {body}"
    );
    assert!(
        body.contains("id=\"board-columns\""),
        "the fragment is the columns container itself: {body}"
    );
}
