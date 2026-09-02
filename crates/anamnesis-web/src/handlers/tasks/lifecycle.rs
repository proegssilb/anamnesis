//! Raising a task above the horizon and dropping it back (`docs/DOMAIN.md`
//! §2, §5's bounce accounting), plus archiving and restoring it.

use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

use anamnesis_app::{archive_task, unarchive_task, view_task};
use anamnesis_core::TaskId;

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use crate::handlers::forms::{CsrfOnlyForm, RaiseTaskForm};

use super::page::render_task_page;
use super::{
    RaiseOutcome, WIP_LIMIT_MESSAGE, drop_task_with_bounce_accounting, raise_task_to_column,
    role_for_task,
};

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

    match raise_task_to_column(state, role, task_id, column_id).await? {
        RaiseOutcome::Raised => Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response()),
        RaiseOutcome::WipLimitExceeded => {
            let aggregate = view_task(state.tasks.as_ref(), role, task_id).await?;
            // The placement editor is a `#task-placement` modal now, not a
            // `<details>` — it reopens itself because the raise form's
            // `action` carries that fragment, so this render doesn't need an
            // `open_hint` the way title/description edits still do.
            render_task_page(
                state,
                user,
                task_id,
                &aggregate.task,
                Some(WIP_LIMIT_MESSAGE),
                None,
                None,
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await
        }
    }
}

pub async fn drop_task_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<CsrfOnlyForm>,
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
    form: CsrfOnlyForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    drop_task_with_bounce_accounting(state, role, task_id).await?;
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
