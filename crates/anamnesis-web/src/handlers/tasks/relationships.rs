use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use anamnesis_app::{
    AppError, create_relationship, delete_relationship, resolve_kind, view_task,
};
use anamnesis_core::{
    RelationshipId, TaskId, builtin_blocks, builtin_duplicates, builtin_relates_to,
};

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use crate::handlers::forms::{CreateRelationshipForm, CsrfOnlyForm};

use super::candidates::task_candidates_impl;
use super::page::render_task_page;
use super::role_for_task;

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

/// Maps the form's plain-string `kind` to the built-in relationship kind id
/// it names — pure input parsing, kept separate from
/// [`create_relationship_impl`]'s handling of the async `create_relationship`
/// result.
fn relationship_kind_id(kind: &str) -> Result<anamnesis_core::KindId, WebError> {
    match kind {
        "blocks" => Ok(builtin_blocks().id),
        "relates_to" => Ok(builtin_relates_to().id),
        "duplicates" => Ok(builtin_duplicates().id),
        other => Err(WebError::BadRequest(format!(
            "{other:?} is not a known relationship kind"
        ))),
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

    let kind_id = relationship_kind_id(&form.kind)?;
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
                None,
                None,
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

#[derive(Debug, Deserialize)]
pub struct RelationshipCandidatesParams {
    #[serde(default)]
    pub q: String,
}

/// Backs the relationship-target picker (`templates/task.html`'s "Add
/// relationship" modal): a title search structurally identical to the
/// parent picker (`super::hierarchy::parent_candidates_impl`), scoped down
/// to `SearchHit::Task` and with this task's own id dropped (a task can't
/// relate to itself — `create_relationship_impl` would reject it anyway).
/// Unlike the parent picker, selecting a candidate here can't self-submit:
/// creating a relationship also needs a `kind`, so each result is a plain
/// "Select" link that round-trips back to the task page with `?rel_to=<id>`
/// and lets `view_task_impl` prefill the relationship form instead.
pub async fn relationship_candidates_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    headers: HeaderMap,
    Query(params): Query<RelationshipCandidatesParams>,
) -> Response {
    match relationship_candidates_impl(&state, &user, TaskId::new(id), &headers, params.q).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn relationship_candidates_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    headers: &HeaderMap,
    query: String,
) -> Result<Response, WebError> {
    task_candidates_impl(
        state,
        user,
        task_id,
        headers,
        query,
        "_relationship_candidates.html",
        "relationship_picker.html",
    )
    .await
}
