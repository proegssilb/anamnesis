//! The HTTP surface of the multi-request (chunked) upload flow
//! (`anamnesis_app::{begin_file_upload, upload_file_part,
//! complete_file_upload, abort_file_upload}`, issue #21). A small JSON API
//! meant to be driven by `fetch()` (`static/chunked-upload.js`), not a plain
//! HTML form — a raw request body has no field to carry a form-encoded CSRF
//! token in, so every route here reads it from an `X-Csrf-Token` header
//! instead of a `csrf_token` form/body field.
//!
//! Only [`begin_upload_handler`] is nested under `/tasks/{id}/...`, because
//! it is the one call with no upload yet to resolve a task from. Every
//! other route is addressed purely by `upload_id` and resolves its task
//! (and therefore its role) from the loaded [`PendingUpload`] row first —
//! the same load-then-authorize shape
//! `super::attachments::download_attachment_impl` already uses for
//! downloads.

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};

use anamnesis_app::{
    AppError, ByteStream, NewUpload, PendingUpload, abort_file_upload, begin_file_upload,
    complete_file_upload, upload_file_part,
};
use anamnesis_core::TaskId;

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::attachments::filename_is_safe;
use super::role_for_task;

fn csrf_from_headers(headers: &HeaderMap) -> &str {
    headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
}

fn check_csrf(user: &CurrentUser, headers: &HeaderMap) -> Result<(), WebError> {
    if csrf_tokens_match(&user.csrf_token, csrf_from_headers(headers)) {
        Ok(())
    } else {
        Err(WebError::CsrfMismatch)
    }
}

/// Loads the pending upload named by `id` and resolves the role its owning
/// task grants `user` — the shared first step for every route below that
/// takes an `upload_id` rather than a `task_id`.
async fn load_upload_and_role(
    state: &AppState,
    user: &CurrentUser,
    id: uuid::Uuid,
) -> Result<(PendingUpload, Option<anamnesis_core::policy::Role>), WebError> {
    let upload_id = anamnesis_app::AttachmentUploadId::new(id);
    let upload = state
        .attachment_uploads
        .load(upload_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let (_, role) = role_for_task(state, &user.user_id, upload.task_id).await?;
    Ok((upload, role))
}

#[derive(Deserialize)]
pub struct BeginUploadRequest {
    filename: String,
    #[serde(default)]
    mime: Option<String>,
}

#[derive(Serialize)]
struct BeginUploadResponse {
    upload_id: uuid::Uuid,
}

pub async fn begin_upload_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    headers: HeaderMap,
    Json(body): Json<BeginUploadRequest>,
) -> Response {
    match begin_upload_impl(&state, &user, TaskId::new(id), &headers, body).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn begin_upload_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    headers: &HeaderMap,
    body: BeginUploadRequest,
) -> Result<Response, WebError> {
    check_csrf(user, headers)?;
    if !filename_is_safe(&body.filename) {
        return Err(WebError::BadRequest(
            "that filename is not allowed".to_string(),
        ));
    }
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    let mime = body
        .mime
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let upload = begin_file_upload(
        state.attachment_uploads.as_ref(),
        state.chunked.as_ref(),
        state.id_gen.as_ref(),
        state.clock.as_ref(),
        role,
        NewUpload {
            task_id,
            created_by: user.user_id.clone(),
            filename: &body.filename,
            mime: &mime,
        },
    )
    .await?;
    Ok(Json(BeginUploadResponse {
        upload_id: upload.id.as_uuid(),
    })
    .into_response())
}

pub async fn upload_part_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, part_number)): Path<(uuid::Uuid, u32)>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    match upload_part_impl(&state, &user, id, part_number, &headers, body).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

/// Wraps `body`'s stream so that once more than `limit` bytes have been
/// read, it ends with a [`crate::error::BodyTooLarge`] error instead of
/// continuing to grow unbounded.
///
/// `axum::body::Body` consumed directly like this bypasses
/// `DefaultBodyLimit` entirely — that layer only wraps `Bytes`-based
/// extractors (`Json`, `Form`, `Multipart`'s own field reads), per its own
/// doc comment: "if an extractor consumes the body directly with
/// `Body::poll_frame`, or similar, the default limit is not applied." A
/// route that streams the raw body, like this one, needs its own check or
/// `ANAMNESIS_MAX_BODY_BYTES` would silently stop bounding it.
fn limited_body_stream(body: Body, limit: usize) -> ByteStream<'static> {
    let mut seen: usize = 0;
    let stream = body.into_data_stream().map(move |chunk| {
        let chunk = chunk.map_err(std::io::Error::other)?;
        seen += chunk.len();
        if seen > limit {
            return Err(std::io::Error::other(crate::error::BodyTooLarge));
        }
        Ok(chunk)
    });
    Box::pin(stream)
}

async fn upload_part_impl(
    state: &AppState,
    user: &CurrentUser,
    id: uuid::Uuid,
    part_number: u32,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, WebError> {
    check_csrf(user, headers)?;
    let (upload, role) = load_upload_and_role(state, user, id).await?;
    let data = limited_body_stream(body, state.max_body_bytes);
    upload_file_part(
        state.attachment_uploads.as_ref(),
        state.chunked.as_ref(),
        role,
        state.max_attachment_bytes,
        upload.id,
        part_number,
        data,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Serialize)]
struct CompleteUploadResponse {
    task_id: uuid::Uuid,
    attachment_id: uuid::Uuid,
}

pub async fn complete_upload_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    headers: HeaderMap,
) -> Response {
    match complete_upload_impl(&state, &user, id, &headers).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn complete_upload_impl(
    state: &AppState,
    user: &CurrentUser,
    id: uuid::Uuid,
    headers: &HeaderMap,
) -> Result<Response, WebError> {
    check_csrf(user, headers)?;
    let (upload, role) = load_upload_and_role(state, user, id).await?;
    let attachment = complete_file_upload(
        state.attachments.as_ref(),
        state.attachment_uploads.as_ref(),
        state.chunked.as_ref(),
        state.id_gen.as_ref(),
        state.clock.as_ref(),
        role,
        upload.id,
    )
    .await?;
    Ok(Json(CompleteUploadResponse {
        task_id: attachment.task_id.as_uuid(),
        attachment_id: attachment.id.as_uuid(),
    })
    .into_response())
}

pub async fn abort_upload_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    headers: HeaderMap,
) -> Response {
    match abort_upload_impl(&state, &user, id, &headers).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn abort_upload_impl(
    state: &AppState,
    user: &CurrentUser,
    id: uuid::Uuid,
    headers: &HeaderMap,
) -> Result<Response, WebError> {
    check_csrf(user, headers)?;
    let (upload, role) = load_upload_and_role(state, user, id).await?;
    abort_file_upload(
        state.attachment_uploads.as_ref(),
        state.chunked.as_ref(),
        role,
        upload.id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
