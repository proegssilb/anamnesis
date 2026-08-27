//! Task detail: fields, relationships, checklist, comments, attachments
//! (`docs/DOMAIN.md` §8) — plus raising a task above the horizon and
//! dropping it back (§2, §5's bounce accounting).

use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use minijinja::context;

use anamnesis_app::{
    AppError, add_comment, add_link_attachment, create_relationship, delete_relationship,
    drop_task, edit_task, list_attachments, list_comments, raise_task, resolve_kind,
    set_task_parent, view_task,
};
use anamnesis_core::{
    Placement, RelationshipId, TaskId, builtin_blocks, builtin_duplicates, builtin_relates_to,
};

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::access;
use super::format::column_is_done;
use super::forms::{
    AddCommentForm, AddLinkAttachmentForm, CreateRelationshipForm, CsrfOnlyForm, EditTaskForm,
    RaiseTaskForm, SetParentForm,
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
        role,
        task_id,
        &form.title,
        &form.description,
    )
    .await
    {
        Ok(task) => render_task_page(state, user, task_id, &task, None, StatusCode::OK).await,
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
