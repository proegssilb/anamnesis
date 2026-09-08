use axum::Form;
use axum::body::Body;
use axum::extract::multipart::MultipartError;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use futures_util::StreamExt as _;

use anamnesis_app::{
    AppError, AttachmentId, AttachmentKind, ByteStream, add_file_attachment, add_link_attachment,
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

/// Uploads a file and attaches it to a task, through
/// `anamnesis_app::add_file_attachment` and the configured `BlobStore`
/// (`docs/DOMAIN.md` §3: "Files need a new `BlobStore` port"). A
/// `multipart/form-data` POST — the one mutating route in this crate that is
/// not a plain URL-encoded form, since a file upload has no URL-encoded
/// shape. The file field's bytes are streamed straight into the blob store
/// as they arrive off the socket — nothing in this handler ever holds a
/// whole attachment in memory (`anamnesis_app::BlobStore`'s doc comment has
/// the full design).
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

/// Translates a multipart read failure into the right `WebError`.
///
/// The router-wide `ANAMNESIS_MAX_BODY_BYTES` limit
/// (`crate::routes::build_router`) is enforced by a layer *underneath* this
/// extractor, so exceeding it arrives here as an ordinary `MultipartError`.
/// Reporting that as a 400 would tell an uploader their file was malformed
/// when it was merely too big, so `status()` -- which axum sets to 413 for
/// exactly that case -- decides which it was.
fn multipart_error(err: MultipartError) -> WebError {
    if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return WebError::PayloadTooLarge(
            "that upload is larger than this server accepts".to_string(),
        );
    }
    WebError::BadRequest(format!("malformed upload: {err}"))
}

/// Validates a found `"file"` field's headers and CSRF state, immediately
/// before its bytes are streamed anywhere — split out of
/// [`add_file_attachment_impl`] purely to keep that function's own
/// complexity within `just lizard`'s budget; it has no meaning on its own.
fn validate_file_field(
    csrf_token: &str,
    user_csrf_token: &str,
    filename: Option<&str>,
) -> Result<String, WebError> {
    if !csrf_tokens_match(user_csrf_token, csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let filename = filename
        .filter(|f| !f.is_empty())
        .ok_or_else(|| WebError::BadRequest("no file was chosen".to_string()))?;
    if !filename_is_safe(filename) {
        return Err(WebError::BadRequest(
            "that filename is not allowed".to_string(),
        ));
    }
    Ok(filename.to_string())
}

/// Reads fields one at a time and, on finding `"file"`, streams its bytes
/// straight into [`add_file_attachment`] — there is deliberately no
/// intermediate step that hands a field's data back out of this function
/// (e.g. as a struct holding a [`ByteStream`]): a `Field<'_>` borrows
/// `&mut Multipart` for as long as its data is read, and returning one from
/// a helper — through a return type whose lifetime is tied to the *whole*
/// `Multipart` borrow — makes every other field-handling branch look, to
/// the borrow checker, like it also needs that same unbounded borrow (a
/// known limitation of today's non-Polonius borrow checker: a value's
/// region is inferred from the union of every branch's requirements, not
/// per-branch). Consuming the stream to completion right here, in the same
/// scope it was created, needs no such region — the file's bytes never
/// have to outlive this one loop iteration.
///
/// Stopping at `"file"` rather than continuing to drain the rest of the
/// body is not just an optimisation: it means a `csrf_token` field placed
/// *after* `file` is never observed, so `csrf_token` stays empty here and
/// [`validate_file_field`] rejects the request — a client cannot reorder
/// fields to slip a file past CSRF validation. The real upload form
/// (`templates/task.html`) emits `csrf_token` before `file`, so this
/// matches how every legitimate request is already shaped.
///
/// There is deliberately no per-file size cap here beyond the router-wide
/// `ANAMNESIS_MAX_BODY_BYTES` layer — see that constant's own doc comment
/// for why a second, narrower cap does not (yet) earn its keep.
async fn add_file_attachment_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    mut multipart: Multipart,
) -> Result<Response, WebError> {
    let mut csrf_token = String::new();

    while let Some(field) = multipart.next_field().await.map_err(multipart_error)? {
        match field.name().unwrap_or_default() {
            "csrf_token" => {
                csrf_token = field.text().await.map_err(multipart_error)?;
            }
            "file" => {
                let filename =
                    validate_file_field(&csrf_token, &user.csrf_token, field.file_name())?;
                let mime = field
                    .content_type()
                    .map(str::to_string)
                    .unwrap_or_else(|| "application/octet-stream".to_string());
                let data: ByteStream<'_> =
                    Box::pin(field.map(|r| r.map_err(std::io::Error::other)));

                let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
                add_file_attachment(
                    state.attachments.as_ref(),
                    state.blobs.as_ref(),
                    state.id_gen.as_ref(),
                    state.clock.as_ref(),
                    role,
                    task_id,
                    &filename,
                    &mime,
                    data,
                )
                .await?;
                return Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response());
            }
            _ => {}
        }
    }

    Err(WebError::BadRequest("no file was chosen".to_string()))
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
    headers: HeaderMap,
) -> Response {
    match download_attachment_impl(&state, &user, AttachmentId::new(id), &headers).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

/// Parses a `Range: bytes=...` request header against a known total `size`,
/// returning the single satisfiable byte range (inclusive bounds) to serve.
///
/// `None` covers every case that falls back to an ordinary full response:
/// no header, a header that isn't `bytes=`-prefixed, a multi-range request
/// (`bytes=0-10,20-30` — unsupported, single-range is the standard,
/// well-supported simplification), or a malformed number (RFC 9110 §14.1.2:
/// a server ignoring an unparseable Range header and serving the whole
/// representation is correct behaviour, not an error). `Err` is reserved for
/// a syntactically valid range this attachment's size cannot satisfy at
/// all — a first-byte position at or past `size`, or a zero-length suffix —
/// which becomes a 416.
fn parse_single_range(header: Option<&str>, size: u64) -> Result<Option<(u64, u64)>, WebError> {
    let Some(spec) = header.and_then(|h| h.strip_prefix("bytes=")) else {
        return Ok(None);
    };
    if spec.contains(',') {
        return Ok(None);
    }
    let Some((start, end)) = spec.split_once('-') else {
        return Ok(None);
    };

    if start.is_empty() {
        return suffix_range(end, size);
    }
    let Ok(start) = start.parse::<u64>() else {
        return Ok(None);
    };
    if start >= size {
        return Err(WebError::RangeNotSatisfiable(size));
    }
    let end = match end.is_empty() {
        true => size - 1,
        false => match end.parse::<u64>() {
            Ok(e) if e >= start => e.min(size - 1),
            _ => return Ok(None),
        },
    };
    Ok(Some((start, end)))
}

/// The `bytes=-N` ("last N bytes") arm of [`parse_single_range`], split out
/// to keep that function's branching within `just lizard`'s budget.
fn suffix_range(suffix: &str, size: u64) -> Result<Option<(u64, u64)>, WebError> {
    let Ok(suffix) = suffix.parse::<u64>() else {
        return Ok(None);
    };
    if suffix == 0 {
        return Err(WebError::RangeNotSatisfiable(size));
    }
    // A suffix longer than the object just means "the whole thing" (RFC
    // 9110 §14.1.2), not an error.
    let suffix = suffix.min(size);
    Ok(Some((size - suffix, size - 1)))
}

async fn download_attachment_impl(
    state: &AppState,
    user: &CurrentUser,
    attachment_id: AttachmentId,
    headers: &HeaderMap,
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
        size,
    } = &attachment.kind
    else {
        return Err(WebError::BadRequest(
            "that attachment has no file to download".to_string(),
        ));
    };
    let size = *size;
    let (_, role) = role_for_task(state, &user.user_id, attachment.task_id).await?;
    // `list_attachments`'s own gate (`Action::ViewTask`) is exactly what a
    // download should be gated on — reused here via the use case itself
    // rather than re-deriving the check, keeping this one call site honest
    // about going through the same permission path every other read of this
    // task's attachments does.
    list_attachments(state.attachments.as_ref(), role, attachment.task_id).await?;

    let range_header = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    match parse_single_range(range_header, size) {
        Ok(None) => {
            let body = state.blobs.get(blob_key).await?.ok_or(AppError::NotFound)?;
            Ok(full_response(mime, filename, size, body))
        }
        Ok(Some((start, end))) => {
            let body = state
                .blobs
                .get_range(blob_key, start, end)
                .await?
                .ok_or(AppError::NotFound)?;
            Ok(partial_response(mime, filename, size, start, end, body))
        }
        Err(WebError::RangeNotSatisfiable(size)) => Ok(range_not_satisfiable_response(size)),
        Err(e) => Err(e),
    }
}

fn content_disposition(filename: &str) -> String {
    format!("attachment; filename=\"{}\"", filename.replace('"', "'"))
}

fn full_response(mime: &str, filename: &str, size: u64, body: ByteStream<'static>) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CONTENT_DISPOSITION, content_disposition(filename)),
            (header::CONTENT_LENGTH, size.to_string()),
            (header::ACCEPT_RANGES, "bytes".to_string()),
        ],
        Body::from_stream(body),
    )
        .into_response()
}

fn partial_response(
    mime: &str,
    filename: &str,
    size: u64,
    start: u64,
    end: u64,
    body: ByteStream<'static>,
) -> Response {
    (
        StatusCode::PARTIAL_CONTENT,
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CONTENT_DISPOSITION, content_disposition(filename)),
            (header::CONTENT_LENGTH, (end - start + 1).to_string()),
            (header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}")),
            (header::ACCEPT_RANGES, "bytes".to_string()),
        ],
        Body::from_stream(body),
    )
        .into_response()
}

fn range_not_satisfiable_response(size: u64) -> Response {
    (
        StatusCode::RANGE_NOT_SATISFIABLE,
        [(header::CONTENT_RANGE, format!("bytes */{size}"))],
    )
        .into_response()
}
