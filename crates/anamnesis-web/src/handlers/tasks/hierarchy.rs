//! The checklist/parent hierarchy: setting or clearing a task's parent, the
//! title-search picker that backs that form, and quick-adding a checklist
//! item (which is just create-a-task-then-set-its-parent in one request).

use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use anamnesis_app::{AppError, create_task, set_checklist_position, set_task_parent, view_task};
use anamnesis_core::TaskId;

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use crate::handlers::forms::{AddChecklistItemForm, SetParentForm};

use super::candidates::task_candidates_impl;
use super::page::render_task_page;
use super::role_for_task;

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
                None,
                None,
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await
        }
        Err(err) => Err(WebError::from(err)),
    }
}

#[derive(Debug, Deserialize)]
pub struct ParentCandidatesParams {
    #[serde(default)]
    pub q: String,
}

/// Backs the parent-task picker (`templates/task.html`'s "Change parent"
/// panel): a title search over `anamnesis_app::SearchQuery`, the same port
/// `crate::handlers::search` uses, scoped down to `SearchHit::Task` and with
/// this task's own id dropped from the results (it can never be its own
/// parent — `anamnesis_core::task::set_parent` would reject it anyway, but
/// there is no reason to ever list it as a candidate). Gated on the same
/// `view_task` permission the task page itself requires, since the picker is
/// reachable only from there; the candidate titles it surfaces from other
/// projects carry the same trade-off `crate::handlers::search`'s own doc
/// comment names for global search — a title, naming nothing beyond what its
/// own page already reveals to whoever can reach it.
pub async fn parent_candidates_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    headers: HeaderMap,
    Query(params): Query<ParentCandidatesParams>,
) -> Response {
    match parent_candidates_impl(&state, &user, TaskId::new(id), &headers, params.q).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn parent_candidates_impl(
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
        "_parent_candidates.html",
        "parent_picker.html",
    )
    .await
}

/// Quick-adds a checklist item: creates a new task in this task's project
/// and appends it as a child, in one request (`forms::AddChecklistItemForm`'s
/// doc comment) — the create and the parent-link are two separate use-case
/// calls, but only the first can meaningfully fail on user input (a blank or
/// too-long title), so only it gets an inline re-render on
/// `AppError::Rule`; a failure in the link-up or reposition step after that
/// is already-a-server-error territory and just bubbles up.
pub async fn add_checklist_item_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<AddChecklistItemForm>,
) -> Response {
    match add_checklist_item_impl(&state, &user, TaskId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn add_checklist_item_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    form: AddChecklistItemForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    let parent = state.tasks.load(task_id).await?.ok_or(AppError::NotFound)?;
    let next_position = next_checklist_position(state, task_id).await?;

    let new_task = match create_task(
        state.tasks.as_ref(),
        state.id_gen.as_ref(),
        state.clock.as_ref(),
        state.search_index.as_ref(),
        role,
        parent.task.project_id,
        &form.title,
        "",
    )
    .await
    {
        Ok(task) => task,
        Err(AppError::Rule(e)) => {
            let aggregate = view_task(state.tasks.as_ref(), role, task_id).await?;
            return render_task_page(
                state,
                user,
                task_id,
                &aggregate.task,
                Some(&e.to_string()),
                None,
                None,
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await;
        }
        Err(err) => return Err(WebError::from(err)),
    };

    link_checklist_item(state, role, new_task.id, task_id, next_position).await?;

    Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response())
}

/// The next free checklist position under `task_id` — one past the highest
/// position among its existing children, or `0` if it has none yet.
async fn next_checklist_position(state: &AppState, task_id: TaskId) -> Result<u32, WebError> {
    let existing_siblings = state.tasks.list_children(task_id).await?;
    Ok(existing_siblings
        .iter()
        .map(|t| t.checklist_position)
        .max()
        .map_or(0, |max| max + 1))
}

/// Links a freshly created task in as `parent_id`'s checklist child, and —
/// only when it isn't already first — repositions it to `position`. Split
/// out of [`add_checklist_item_impl`] so the create-then-optionally-reposition
/// sequence reads as one step there instead of three inline calls.
async fn link_checklist_item(
    state: &AppState,
    role: Option<anamnesis_core::policy::Role>,
    child_id: TaskId,
    parent_id: TaskId,
    position: u32,
) -> Result<(), WebError> {
    set_task_parent(
        state.tasks.as_ref(),
        state.clock.as_ref(),
        role,
        child_id,
        Some(parent_id),
    )
    .await?;
    if position > 0 {
        set_checklist_position(state.tasks.as_ref(), role, child_id, position).await?;
    }
    Ok(())
}
