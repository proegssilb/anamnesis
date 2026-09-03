//! `tower::ServiceExt::oneshot` coverage for file attachments
//! (`docs/DOMAIN.md` §3): `add_file_attachment` was fully implemented in
//! `anamnesis-app` and backed by `anamnesis_adapters::FsBlobStore`, but had
//! no HTTP route before this phase. Covers the upload-then-download round
//! trip, the size limit, and the traversal-shaped-filename rejection
//! `crate::handlers::tasks::filename_is_safe`'s doc comment promises.

mod support;

use axum::http::StatusCode;

use support::{TestApp, body_text, location_of};

async fn setup_task(app: &TestApp) -> String {
    let area_path = location_of(
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
    let project_path = location_of(
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
    location_of(
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

#[tokio::test]
async fn a_file_round_trips_through_upload_and_download() {
    let app = TestApp::new(true).await;
    let task_path = setup_task(&app).await;

    let upload = app
        .post_multipart(
            &format!("{task_path}/attachments/file"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            (
                "file",
                "inspection-report.txt",
                b"roof needs work",
                "text/plain",
            ),
            None,
        )
        .await;
    assert_eq!(upload.status(), StatusCode::SEE_OTHER);
    assert_eq!(location_of(&upload), task_path);

    let task_body = body_text(app.get(&task_path, None).await).await;
    assert!(
        task_body.contains("inspection-report.txt"),
        "the uploaded filename must appear on the task page: {task_body}"
    );

    // Anchor on the actual download link's `/download` suffix rather than
    // the first `/attachments/` substring on the page: the task page also
    // links to the "Add attachment" modal, whose file-upload form posts to
    // `/tasks/{id}/attachments/file` — a `/attachments/` occurrence that can
    // land earlier in the markup than the download link itself.
    let download_marker = "/attachments/";
    let end = task_body
        .find("/download")
        .expect("a download link must be rendered");
    let before = &task_body[..end];
    let start = before
        .rfind(download_marker)
        .expect("a download link must be rendered")
        + download_marker.len();
    let attachment_id = &before[start..];

    let download = app
        .get(&format!("/attachments/{attachment_id}/download"), None)
        .await;
    assert_eq!(download.status(), StatusCode::OK);
    let bytes = download.into_body();
    let body = http_body_util::BodyExt::collect(bytes)
        .await
        .unwrap()
        .to_bytes();
    assert_eq!(&body[..], b"roof needs work");
}

#[tokio::test]
async fn an_oversized_file_is_rejected() {
    let app = TestApp::new(true).await;
    let task_path = setup_task(&app).await;

    // One byte over the 10 MiB limit
    // (`crate::handlers::tasks::MAX_ATTACHMENT_BYTES`).
    let huge = vec![0u8; 10 * 1024 * 1024 + 1];
    let upload = app
        .post_multipart(
            &format!("{task_path}/attachments/file"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            ("file", "huge.bin", &huge, "application/octet-stream"),
            None,
        )
        .await;
    assert!(
        upload.status().is_client_error(),
        "an oversized upload must be rejected, got {}",
        upload.status()
    );

    // Nothing was stored.
    let task_body = body_text(app.get(&task_path, None).await).await;
    assert!(!task_body.contains("huge.bin"));
}

#[tokio::test]
async fn a_traversal_shaped_filename_is_rejected() {
    let app = TestApp::new(true).await;
    let task_path = setup_task(&app).await;

    let upload = app
        .post_multipart(
            &format!("{task_path}/attachments/file"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            ("file", "../../etc/passwd", b"pwned", "text/plain"),
            None,
        )
        .await;
    assert!(
        upload.status().is_client_error(),
        "a traversal-shaped filename must be rejected, got {}",
        upload.status()
    );

    // Confirm nothing escaped the configured blob root either, as a second
    // line of defense (`anamnesis_adapters::FsBlobStore`'s own guard).
    assert!(
        !app.blob_root
            .parent()
            .map(|p| p.join("etc/passwd").exists())
            .unwrap_or(false)
    );
}

#[tokio::test]
async fn upload_without_a_valid_csrf_token_is_rejected() {
    let app = TestApp::new(true).await;
    let task_path = setup_task(&app).await;

    let upload = app
        .post_multipart(
            &format!("{task_path}/attachments/file"),
            &[("csrf_token", "wrong-token")],
            ("file", "note.txt", b"hello", "text/plain"),
            None,
        )
        .await;
    assert_eq!(upload.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn upload_by_an_ungranted_user_is_forbidden() {
    let (app, task_path, stranger_cookie) = support::setup_task_as_admin().await;

    let upload = app
        .post_multipart(
            &format!("{task_path}/attachments/file"),
            &[("csrf_token", "stranger-token")],
            ("file", "note.txt", b"hello", "text/plain"),
            Some(&stranger_cookie),
        )
        .await;
    assert_eq!(upload.status(), StatusCode::FORBIDDEN);
}
