//! Task detail: fields, relationships, checklist, comments, attachments
//! (`docs/DOMAIN.md` §8) — plus raising a task above the horizon and
//! dropping it back (§2, §5's bounce accounting).

use axum::Form;
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use minijinja::context;

use anamnesis_app::{
    AppError, AttachmentId, AttachmentKind, add_comment, add_file_attachment, add_link_attachment,
    archive_task, create_relationship, delete_relationship, drop_task, edit_task, list_attachments,
    list_comments, raise_task, resolve_kind, set_task_field_value, set_task_parent, unarchive_task,
    view_task,
};
use anamnesis_core::{
    FieldId, Placement, RelationshipId, TaskId, builtin_blocks, builtin_duplicates,
    builtin_relates_to,
};

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::access;
use super::field_form;
use super::format::{column_is_done, field_input_value, format_field_data, format_field_kind};
use super::forms::{
    AddCommentForm, AddLinkAttachmentForm, CreateRelationshipForm, CsrfOnlyForm, EditTaskForm,
    RaiseTaskForm, SetFieldValueForm, SetParentForm,
};

/// Resolves the role a task's own project grants `user` — every task
/// handler needs this once, up front, to gate the actual use case call.
pub(super) async fn role_for_task(
    state: &AppState,
    user_id: &anamnesis_core::UserId,
    task_id: TaskId,
) -> Result<
    (
        anamnesis_core::ProjectId,
        Option<anamnesis_core::policy::Role>,
    ),
    WebError,
> {
    let aggregate = state.tasks.load(task_id).await?.ok_or(AppError::NotFound)?;
    let project_id = aggregate.task.project_id;
    let project = state
        .projects
        .load(project_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let role = access::project_role(state, user_id, project_id, project.project.area_id).await?;
    Ok((project_id, role))
}

pub async fn view_task_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    match view_task_impl(&state, &user, TaskId::new(id)).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn view_task_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
) -> Result<Response, WebError> {
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    let aggregate = view_task(state.tasks.as_ref(), role, task_id).await?;
    render_task_page(state, user, task_id, &aggregate.task, None, StatusCode::OK).await
}

pub async fn edit_task_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<EditTaskForm>,
) -> Response {
    match edit_task_impl(&state, &user, TaskId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn edit_task_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    form: EditTaskForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    match edit_task(
        state.tasks.as_ref(),
        state.clock.as_ref(),
        state.search_index.as_ref(),
        role,
        task_id,
        &form.title,
        &form.description,
    )
    .await
    {
        Ok(task) => {
            // Re-indexed inside `edit_task` itself — see
            // `anamnesis_app::use_cases::indexing`'s module doc comment.
            render_task_page(state, user, task_id, &task, None, StatusCode::OK).await
        }
        Err(AppError::Rule(e)) => {
            let aggregate = view_task(state.tasks.as_ref(), role, task_id).await?;
            render_task_page(
                state,
                user,
                task_id,
                &aggregate.task,
                Some(&e.to_string()),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await
        }
        Err(err) => Err(WebError::from(err)),
    }
}

pub async fn raise_task_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<RaiseTaskForm>,
) -> Response {
    match raise_task_impl(&state, &user, TaskId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn raise_task_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    form: RaiseTaskForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    let column_id = anamnesis_core::ColumnId::new(form.column_id);
    let position = state.board.count_on_column(column_id).await?;

    match raise_task(
        state.tasks.as_ref(),
        state.board.as_ref(),
        state.clock.as_ref(),
        role,
        task_id,
        column_id,
        position,
    )
    .await
    {
        Ok(_) => Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response()),
        Err(AppError::WipLimitExceeded) => {
            let aggregate = view_task(state.tasks.as_ref(), role, task_id).await?;
            render_task_page(
                state,
                user,
                task_id,
                &aggregate.task,
                Some("That column is already at its work-in-progress limit."),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await
        }
        Err(err) => Err(WebError::from(err)),
    }
}

pub async fn drop_task_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<super::forms::CsrfOnlyForm>,
) -> Response {
    match drop_task_impl(&state, &user, TaskId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn drop_task_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    form: super::forms::CsrfOnlyForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    let aggregate = state.tasks.load(task_id).await?.ok_or(AppError::NotFound)?;
    let left_a_done_column = match aggregate.task.placement {
        Placement::OnBoard { column, .. } => {
            let columns = state.board.columns_with_items().await?;
            column_is_done(&columns, column).unwrap_or(false)
        }
        Placement::Below => false,
    };

    drop_task(
        state.tasks.as_ref(),
        state.clock.as_ref(),
        role,
        task_id,
        left_a_done_column,
    )
    .await?;
    Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response())
}

/// Archives a task (`docs/DOMAIN.md` §2: "vanished from every view unless
/// explicitly searched").
pub async fn archive_task_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<CsrfOnlyForm>,
) -> Response {
    match archive_task_impl(&state, &user, TaskId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn archive_task_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    form: CsrfOnlyForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    archive_task(
        state.tasks.as_ref(),
        state.clock.as_ref(),
        state.search_index.as_ref(),
        role,
        task_id,
    )
    .await?;
    Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response())
}

/// Restores an archived task.
pub async fn unarchive_task_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<CsrfOnlyForm>,
) -> Response {
    match unarchive_task_impl(&state, &user, TaskId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn unarchive_task_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    form: CsrfOnlyForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    unarchive_task(
        state.tasks.as_ref(),
        state.clock.as_ref(),
        state.search_index.as_ref(),
        role,
        task_id,
    )
    .await?;
    Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response())
}

pub async fn add_comment_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<AddCommentForm>,
) -> Response {
    match add_comment_impl(&state, &user, TaskId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn add_comment_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    form: AddCommentForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    add_comment(
        state.comments.as_ref(),
        state.id_gen.as_ref(),
        state.clock.as_ref(),
        role,
        task_id,
        user.user_id.clone(),
        &form.body,
    )
    .await?;
    Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response())
}

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

async fn add_file_attachment_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    mut multipart: Multipart,
) -> Result<Response, WebError> {
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

    let csrf_token = csrf_token.unwrap_or_default();
    if !csrf_tokens_match(&user.csrf_token, &csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let filename = filename
        .filter(|f| !f.is_empty())
        .ok_or_else(|| WebError::BadRequest("no file was chosen".to_string()))?;
    if !filename_is_safe(&filename) {
        return Err(WebError::BadRequest(
            "that filename is not allowed".to_string(),
        ));
    }
    let bytes = bytes.ok_or_else(|| WebError::BadRequest("no file was chosen".to_string()))?;

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

pub async fn set_parent_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<SetParentForm>,
) -> Response {
    match set_parent_impl(&state, &user, TaskId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn set_parent_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    form: SetParentForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    let trimmed = form.parent_task_id.trim();
    let new_parent = if trimmed.is_empty() {
        None
    } else {
        let raw = uuid::Uuid::parse_str(trimmed)
            .map_err(|_| WebError::BadRequest("that is not a valid task id".to_string()))?;
        Some(TaskId::new(raw))
    };

    match set_task_parent(
        state.tasks.as_ref(),
        state.clock.as_ref(),
        role,
        task_id,
        new_parent,
    )
    .await
    {
        Ok(_) => Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response()),
        Err(AppError::Rule(e)) => {
            let aggregate = view_task(state.tasks.as_ref(), role, task_id).await?;
            render_task_page(
                state,
                user,
                task_id,
                &aggregate.task,
                Some(&e.to_string()),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await
        }
        Err(err) => Err(WebError::from(err)),
    }
}

pub async fn create_relationship_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<CreateRelationshipForm>,
) -> Response {
    match create_relationship_impl(&state, &user, TaskId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn create_relationship_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    form: CreateRelationshipForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let (from_project_id, role) = role_for_task(state, &user.user_id, task_id).await?;
    let to_task_id = TaskId::new(form.to_task_id);
    let to = state
        .tasks
        .load(to_task_id)
        .await?
        .ok_or_else(|| WebError::BadRequest("that target task does not exist".to_string()))?;

    let kind_id = match form.kind.as_str() {
        "blocks" => builtin_blocks().id,
        "relates_to" => builtin_relates_to().id,
        "duplicates" => builtin_duplicates().id,
        other => {
            return Err(WebError::BadRequest(format!(
                "{other:?} is not a known relationship kind"
            )));
        }
    };
    let _ = resolve_kind(state.projects.as_ref(), kind_id).await?; // built-ins always resolve; keeps this call site honest about going through the same lookup create_relationship itself uses.

    match create_relationship(
        state.relationships.as_ref(),
        state.projects.as_ref(),
        state.id_gen.as_ref(),
        state.clock.as_ref(),
        role,
        task_id,
        from_project_id,
        to_task_id,
        to.task.project_id,
        kind_id,
    )
    .await
    {
        Ok(_) => Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response()),
        Err(AppError::Rule(e)) => {
            let aggregate = view_task(state.tasks.as_ref(), role, task_id).await?;
            render_task_page(
                state,
                user,
                task_id,
                &aggregate.task,
                Some(&e.to_string()),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await
        }
        Err(err) => Err(WebError::from(err)),
    }
}

/// Sets a task's value for one of its project's custom fields
/// (`docs/DOMAIN.md` §3) — the form every [`anamnesis_core::FieldKind`]
/// needed and never had before this phase (see `super::field_form`'s module
/// doc comment).
pub async fn set_field_value_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, field_id)): Path<(uuid::Uuid, uuid::Uuid)>,
    Form(form): Form<SetFieldValueForm>,
) -> Response {
    match set_field_value_impl(&state, &user, TaskId::new(id), FieldId::new(field_id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn set_field_value_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    field_id: FieldId,
    form: SetFieldValueForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let (project_id, role) = role_for_task(state, &user.user_id, task_id).await?;
    let project = state
        .projects
        .load(project_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let definition = project
        .field_definitions
        .iter()
        .find(|d| d.id == field_id)
        .ok_or(AppError::NotFound)?;

    let data = match field_form::parse_field_data(
        definition.kind,
        &form.value,
        &form.currency,
        state.timezone.as_ref(),
        &state.timezone_name,
    ) {
        Ok(data) => data,
        Err(WebError::BadRequest(message)) => {
            let aggregate = view_task(state.tasks.as_ref(), role, task_id).await?;
            return render_task_page(
                state,
                user,
                task_id,
                &aggregate.task,
                Some(&message),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await;
        }
        Err(err) => return Err(err),
    };

    match set_task_field_value(state.tasks.as_ref(), role, definition, task_id, data).await {
        Ok(_) => Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response()),
        Err(AppError::Rule(e)) => {
            let aggregate = view_task(state.tasks.as_ref(), role, task_id).await?;
            render_task_page(
                state,
                user,
                task_id,
                &aggregate.task,
                Some(&e.to_string()),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await
        }
        Err(err) => Err(WebError::from(err)),
    }
}

/// Deletes a relationship edge — reachable from either end's task page (the
/// URL's `id` names whichever task the delete form was submitted from, and
/// need only be *one* of the edge's two tasks, not specifically the `from`
/// side; see `delete_relationship_impl`). Permission is checked against
/// that task's own project, exactly like `create_relationship_handler`
/// checks against the initiating task's project — deleting from either
/// listing (forward or reverse) only ever needs a role on the task whose
/// page you are looking at.
pub async fn delete_relationship_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, relationship_id)): Path<(uuid::Uuid, uuid::Uuid)>,
    Form(form): Form<CsrfOnlyForm>,
) -> Response {
    match delete_relationship_impl(
        &state,
        &user,
        TaskId::new(id),
        RelationshipId::new(relationship_id),
        form,
    )
    .await
    {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn delete_relationship_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    relationship_id: RelationshipId,
    form: CsrfOnlyForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;

    // The relationship must actually involve the task named in the URL —
    // otherwise a role on *some* project the caller belongs to would let
    // them delete an edge between two entirely unrelated tasks just by
    // naming its id on their own task's delete route.
    let relationship = state
        .relationships
        .load(relationship_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if relationship.from_task_id != task_id && relationship.to_task_id != task_id {
        return Err(WebError::App(AppError::NotFound));
    }

    delete_relationship(state.relationships.as_ref(), role, relationship_id).await?;
    Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response())
}

/// Assembles and renders the task detail page: the task itself, its
/// checklist children, comments, attachments, relationships (with the other
/// end's title resolved for display), and the board columns available for
/// the raise-task form.
async fn render_task_page(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    task: &anamnesis_core::Task,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, WebError> {
    let children = state.tasks.list_children(task_id).await?;
    let comments = list_comments(state.comments.as_ref(), Some(member_role()), task_id).await?;
    let attachments =
        list_attachments(state.attachments.as_ref(), Some(member_role()), task_id).await?;
    let raw_relationships = state.relationships.list_for_task(task_id).await?;

    let mut relationships = Vec::with_capacity(raw_relationships.len());
    for rel in &raw_relationships {
        let (other_id, forward) = if rel.from_task_id == task_id {
            (rel.to_task_id, true)
        } else {
            (rel.from_task_id, false)
        };
        let kind = resolve_kind(state.projects.as_ref(), rel.kind_id).await?;
        let label = if forward {
            kind.forward_label.as_str().to_string()
        } else {
            kind.reverse_label.as_str().to_string()
        };
        let other_title = state
            .tasks
            .load(other_id)
            .await?
            .map(|a| a.task.title.as_str().to_string())
            .unwrap_or_else(|| "(deleted task)".to_string());
        relationships.push(context! {
            id => rel.id.to_string(),
            label => label,
            other_id => other_id.to_string(),
            other_title => other_title,
        });
    }

    let columns = state.board.columns_with_items().await?;
    let column_options: Vec<_> = columns
        .iter()
        .map(|c| context! { id => c.column.id.to_string(), title => c.column.title.as_str() })
        .collect();
    let current_column_is_done = match task.placement {
        Placement::OnBoard { column, .. } => column_is_done(&columns, column),
        Placement::Below => None,
    };

    // Custom field definitions + this task's own values (`docs/DOMAIN.md`
    // §3): the section that made every field genuinely editable, not just
    // displayed (`super::field_form`'s module doc comment).
    let field_definitions = state
        .projects
        .load(task.project_id)
        .await?
        .map(|a| a.field_definitions)
        .unwrap_or_default();
    let field_values = state
        .tasks
        .load(task_id)
        .await?
        .map(|a| a.field_values)
        .unwrap_or_default();
    let fields: Vec<_> = field_definitions
        .iter()
        .map(|def| {
            let stored = field_values.iter().find(|v| v.field_id == def.id);
            let (input_value, currency_code) = stored
                .map(|v| field_input_value(&v.data, state.timezone.as_ref(), &state.timezone_name))
                .unwrap_or_default();
            context! {
                id => def.id.to_string(),
                name => def.name.as_str(),
                kind => format_field_kind(def.kind),
                show_on_card => def.show_on_card,
                display_value => stored.map(|v| format_field_data(&v.data)),
                input_value => input_value,
                currency_code => currency_code.unwrap_or_default(),
            }
        })
        .collect();

    let tmpl = state
        .templates
        .get_template("task.html")
        .map_err(WebError::template)?;
    let body = tmpl
        .render(context! {
            task => task,
            is_on_board => task.placement.is_on_board(),
            current_column_is_done => current_column_is_done,
            children => children,
            comments => comments,
            attachments => attachments,
            relationships => relationships,
            column_options => column_options,
            fields => fields,
            csrf_token => user.csrf_token,
            current_user => user.display_name,
            error => error,
        })
        .map_err(WebError::template)?;
    Ok((status, Html(body)).into_response())
}

/// The task detail page's own read-side calls (`list_comments`,
/// `list_attachments`) are gated identically to `ViewTask`
/// (`can_view_project`), and by the time `render_task_page` runs, the
/// caller has already succeeded at a stronger check on this exact task
/// (`view_task`/`edit_task`/... all resolve the real effective role and
/// would have failed already) — so a fixed `Member` placeholder here only
/// ever satisfies a gate the caller already cleared, never substitutes for
/// it.
fn member_role() -> anamnesis_core::policy::Role {
    anamnesis_core::policy::Role::Member
}
