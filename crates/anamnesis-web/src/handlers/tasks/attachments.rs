use axum::Form;
use axum::extract::{Multipart, Path, State};
use axum::response::{IntoResponse, Redirect, Response};

use anamnesis_app::{
    AppError, AttachmentId, AttachmentKind, add_file_attachment, add_link_attachment,
    list_attachments,
};
use anamnesis_core::TaskId;

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use crate::handlers::forms::AddLinkAttachmentForm;

use super::role_for_task;

pub async fn add_link_attachment_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<AddLinkAttachmentForm>,
) -> Response {
    match add_link_attachment_impl(&state, &user, TaskId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn add_link_attachment_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    form: AddLinkAttachmentForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    add_link_attachment(
        state.attachments.as_ref(),
        state.id_gen.as_ref(),
        state.clock.as_ref(),
        role,
        task_id,
        &form.url,
    )
    .await?;
    Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response())
}

/// The largest file a single attachment upload may carry — 10 MiB. Chosen as
/// a sensible default for a self-hosted single-user/small-team app storing
/// attachments on local disk; nothing in `docs/DOMAIN.md` names a specific
/// figure.
const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;

/// Uploads a file and attaches it to a task, through
/// `anamnesis_app::add_file_attachment` and the configured `BlobStore`
/// (`docs/DOMAIN.md` §3: "Files need a new `BlobStore` port"). A
/// `multipart/form-data` POST — the one mutating route in this crate that is
/// not a plain URL-encoded form, since a file upload has no URL-encoded
/// shape.
pub async fn add_file_attachment_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    multipart: Multipart,
) -> Response {
    match add_file_attachment_impl(&state, &user, TaskId::new(id), multipart).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

/// A filename is rejected outright (rather than merely sanitised) when it
/// carries any path-traversal shape at all: a path separator (so it can
/// never be read as more than one path component), a leading `.` (catches
/// `.`, `..`, and hidden-file-shaped names), or is empty. This is defense in
/// depth alongside — not a replacement for — `anamnesis_adapters::FsBlobStore`'s
/// own guard (`crate::handlers::tasks`'s module doc comment references this
/// same test): the actual on-disk blob key here is always a fresh id minted
/// by `IdGen` (see `anamnesis_app::add_file_attachment`), never derived from
/// the user-supplied filename, so a traversal-shaped filename could not
/// escape the blob store root even unrejected — but the filename is also
/// stored and later rendered/served as metadata (`AttachmentKind::File`'s own
/// `filename`), so rejecting the shape outright is the honest behaviour
/// rather than silently accepting attacker-controlled path syntax into a
/// display field.
fn filename_is_safe(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

/// One parsed `multipart/form-data` upload's fields, before any validation —
/// [`add_file_attachment_impl`] owns deciding whether these are acceptable.
struct ParsedUpload {
    csrf_token: String,
    filename: Option<String>,
    mime: String,
    bytes: Option<Vec<u8>>,
}

/// Reads every field out of the upload, enforcing only [`MAX_ATTACHMENT_BYTES`]
/// (checked per-field as bytes arrive, rather than after buffering the whole
/// upload). Split out of [`add_file_attachment_impl`] so the field-by-field
/// parsing loop reads as one step, separate from validating and acting on
/// the result.
async fn parse_upload_fields(mut multipart: Multipart) -> Result<ParsedUpload, WebError> {
    let mut csrf_token: Option<String> = None;
    let mut filename: Option<String> = None;
    let mut mime: String = "application/octet-stream".to_string();
    let mut bytes: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| WebError::BadRequest(format!("malformed upload: {e}")))?
    {
        match field.name().unwrap_or_default() {
            "csrf_token" => {
                csrf_token = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| WebError::BadRequest(e.to_string()))?,
                );
            }
            "file" => {
                filename = field.file_name().map(str::to_string);
                if let Some(ct) = field.content_type() {
                    mime = ct.to_string();
                }
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| WebError::BadRequest(e.to_string()))?;
                if data.len() > MAX_ATTACHMENT_BYTES {
                    return Err(WebError::BadRequest(format!(
                        "that file is too large — the limit is {} MiB",
                        MAX_ATTACHMENT_BYTES / (1024 * 1024)
                    )));
                }
                bytes = Some(data.to_vec());
            }
            _ => {}
        }
    }

    Ok(ParsedUpload {
        csrf_token: csrf_token.unwrap_or_default(),
        filename,
        mime,
        bytes,
    })
}

async fn add_file_attachment_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    multipart: Multipart,
) -> Result<Response, WebError> {
    let upload = parse_upload_fields(multipart).await?;
    if !csrf_tokens_match(&user.csrf_token, &upload.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let filename = upload
        .filename
        .filter(|f| !f.is_empty())
        .ok_or_else(|| WebError::BadRequest("no file was chosen".to_string()))?;
    if !filename_is_safe(&filename) {
        return Err(WebError::BadRequest(
            "that filename is not allowed".to_string(),
        ));
    }
    let bytes = upload
        .bytes
        .ok_or_else(|| WebError::BadRequest("no file was chosen".to_string()))?;

    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    add_file_attachment(
        state.attachments.as_ref(),
        state.blobs.as_ref(),
        state.id_gen.as_ref(),
        state.clock.as_ref(),
        role,
        task_id,
        &filename,
        &upload.mime,
        bytes,
    )
    .await?;
    Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response())
}

/// Serves a `File` attachment's bytes back for download — the read side of
/// [`add_file_attachment_handler`]. Gated on the same [`Action::ViewTask`]
/// tier as the task page itself (via [`role_for_task`], resolved from the
/// attachment's own owning task, not the URL), so a download link is exactly
/// as visible as the task page it is embedded in and no more.
pub async fn download_attachment_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    match download_attachment_impl(&state, &user, AttachmentId::new(id)).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn download_attachment_impl(
    state: &AppState,
    user: &CurrentUser,
    attachment_id: AttachmentId,
) -> Result<Response, WebError> {
    let attachment = state
        .attachments
        .load(attachment_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let AttachmentKind::File {
        blob_key,
        filename,
        mime,
        ..
    } = &attachment.kind
    else {
        return Err(WebError::BadRequest(
            "that attachment has no file to download".to_string(),
        ));
    };
    let (_, role) = role_for_task(state, &user.user_id, attachment.task_id).await?;
    // `list_attachments`'s own gate (`Action::ViewTask`) is exactly what a
    // download should be gated on — reused here via the use case itself
    // rather than re-deriving the check, keeping this one call site honest
    // about going through the same permission path every other read of this
    // task's attachments does.
    list_attachments(state.attachments.as_ref(), role, attachment.task_id).await?;

    let bytes = state.blobs.get(blob_key).await?.ok_or(AppError::NotFound)?;
    let disposition = format!("attachment; filename=\"{}\"", filename.replace('"', "'"));
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, mime.clone()),
            (axum::http::header::CONTENT_DISPOSITION, disposition),
        ],
        bytes,
    )
        .into_response())
}
