//! `tower::ServiceExt::oneshot` coverage for the multi-request (chunked)
//! upload flow (`crate::handlers::tasks::chunked_attachments`, issue #21) —
//! kept in its own file, separate from `f3_attachments.rs`'s single-request
//! streaming coverage, mirroring the dedicated BDD feature
//! (`chunked_attachment_upload.feature`) this same flow already has at the
//! use-case layer.

mod support;

use axum::http::StatusCode;
use serde_json::Value;

use support::{TestApp, body_text};

async fn setup_task(app: &TestApp) -> String {
    let area_path = support::location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Home hunting"),
                ("description", ""),
            ],
            None,
        )
        .await,
    )
    .to_string();
    let project_path = support::location_of(
        &app.post_form(
            &format!("{area_path}/projects"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "House shopping"),
                ("description", ""),
            ],
            None,
        )
        .await,
    )
    .to_string();
    support::location_of(
        &app.post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "123 Maple St"),
                ("description", ""),
            ],
            None,
        )
        .await,
    )
    .to_string()
}

async fn json_body(response: axum::response::Response<axum::body::Body>) -> Value {
    let text = body_text(response).await;
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("expected JSON body, got {text:?}: {e}"))
}

async fn begin_upload(app: &TestApp, task_path: &str, filename: &str) -> String {
    let response = app
        .post_json(
            &format!("{task_path}/attachments/file/uploads"),
            &format!(r#"{{"filename":"{filename}","mime":"application/octet-stream"}}"#),
            support::DEV_CSRF_TOKEN,
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK, "begin must succeed");
    json_body(response).await["upload_id"]
        .as_str()
        .expect("begin response carries an upload_id")
        .to_string()
}

#[tokio::test]
async fn a_file_uploaded_across_several_parts_round_trips() {
    let app = TestApp::new(true).await;
    let task_path = setup_task(&app).await;
    let upload_id = begin_upload(&app, &task_path, "walkthrough.mp4").await;

    let part1 = app
        .put_bytes(
            &format!("/attachments/uploads/{upload_id}/parts/1"),
            b"hello ",
            support::DEV_CSRF_TOKEN,
            None,
        )
        .await;
    assert_eq!(part1.status(), StatusCode::NO_CONTENT);

    let part2 = app
        .put_bytes(
            &format!("/attachments/uploads/{upload_id}/parts/2"),
            b"world",
            support::DEV_CSRF_TOKEN,
            None,
        )
        .await;
    assert_eq!(part2.status(), StatusCode::NO_CONTENT);

    let complete = app
        .post_json(
            &format!("/attachments/uploads/{upload_id}/complete"),
            "{}",
            support::DEV_CSRF_TOKEN,
            None,
        )
        .await;
    assert_eq!(complete.status(), StatusCode::OK);
    let complete_body = json_body(complete).await;
    let attachment_id = complete_body["attachment_id"]
        .as_str()
        .expect("complete response carries an attachment_id");

    let task_body = body_text(app.get(&task_path, None).await).await;
    assert!(
        task_body.contains("walkthrough.mp4"),
        "the completed upload's filename must appear on the task page: {task_body}"
    );

    let download = app
        .get(&format!("/attachments/{attachment_id}/download"), None)
        .await;
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(body_text(download).await, "hello world");
}

#[tokio::test]
async fn aborting_a_partial_upload_leaves_no_attachment() {
    let app = TestApp::new(true).await;
    let task_path = setup_task(&app).await;
    let upload_id = begin_upload(&app, &task_path, "walkthrough.mp4").await;

    app.put_bytes(
        &format!("/attachments/uploads/{upload_id}/parts/1"),
        b"partial",
        support::DEV_CSRF_TOKEN,
        None,
    )
    .await;

    let abort = app
        .delete_with_csrf(
            &format!("/attachments/uploads/{upload_id}"),
            support::DEV_CSRF_TOKEN,
            None,
        )
        .await;
    assert_eq!(abort.status(), StatusCode::NO_CONTENT);

    let task_body = body_text(app.get(&task_path, None).await).await;
    assert!(!task_body.contains("walkthrough.mp4"));

    // The upload row itself is gone too -- completing it now must 404
    // rather than silently succeed on stale state.
    let complete = app
        .post_json(
            &format!("/attachments/uploads/{upload_id}/complete"),
            "{}",
            support::DEV_CSRF_TOKEN,
            None,
        )
        .await;
    assert_eq!(complete.status(), StatusCode::NOT_FOUND);
}

/// `ANAMNESIS_MAX_BODY_BYTES` still bounds a single request -- one chunk in
/// this flow -- exactly as it does the single-request upload path.
#[tokio::test]
async fn a_part_over_the_router_body_limit_is_too_large() {
    let app = TestApp::with_max_body_bytes(true, 64 * 1024).await;
    let task_path = setup_task(&app).await;
    let upload_id = begin_upload(&app, &task_path, "huge.bin").await;

    let payload = vec![0u8; 128 * 1024];
    let part = app
        .put_bytes(
            &format!("/attachments/uploads/{upload_id}/parts/1"),
            &payload,
            support::DEV_CSRF_TOKEN,
            None,
        )
        .await;
    assert_eq!(part.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

/// `ANAMNESIS_MAX_ATTACHMENT_BYTES` is a distinct knob from the per-request
/// body limit: a running total that crosses it is rejected even though each
/// individual part stays comfortably under `ANAMNESIS_MAX_BODY_BYTES`.
#[tokio::test]
async fn a_running_total_over_the_attachment_cap_is_rejected() {
    let app = TestApp::with_max_attachment_bytes(true, 10).await;
    let task_path = setup_task(&app).await;
    let upload_id = begin_upload(&app, &task_path, "huge.bin").await;

    let first = app
        .put_bytes(
            &format!("/attachments/uploads/{upload_id}/parts/1"),
            &[0u8; 6],
            support::DEV_CSRF_TOKEN,
            None,
        )
        .await;
    assert_eq!(first.status(), StatusCode::NO_CONTENT);

    let second = app
        .put_bytes(
            &format!("/attachments/uploads/{upload_id}/parts/2"),
            &[0u8; 6],
            support::DEV_CSRF_TOKEN,
            None,
        )
        .await;
    assert_eq!(
        second.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "6 + 6 bytes exceeds the 10 byte attachment cap even though neither \
         part alone was over the router's body limit"
    );

    // The whole upload was aborted, not left half-uploaded.
    let task_body = body_text(app.get(&task_path, None).await).await;
    assert!(!task_body.contains("huge.bin"));
}

#[tokio::test]
async fn begin_without_a_valid_csrf_token_is_rejected() {
    let app = TestApp::new(true).await;
    let task_path = setup_task(&app).await;

    let response = app
        .post_json(
            &format!("{task_path}/attachments/file/uploads"),
            r#"{"filename":"walkthrough.mp4","mime":"video/mp4"}"#,
            "wrong-token",
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn begin_by_an_ungranted_user_is_forbidden() {
    let (app, task_path, stranger_cookie) = support::setup_task_as_admin().await;

    let response = app
        .post_json(
            &format!("{task_path}/attachments/file/uploads"),
            r#"{"filename":"walkthrough.mp4","mime":"video/mp4"}"#,
            "stranger-token",
            Some(&stranger_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
