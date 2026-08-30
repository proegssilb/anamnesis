use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;

use anamnesis_app::{AppError, edit_task, view_task};
use anamnesis_core::TaskId;

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use crate::handlers::forms::{EditTaskDescriptionForm, EditTaskTitleForm};

use super::page::render_task_page;
use super::role_for_task;

pub async fn edit_task_title_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<EditTaskTitleForm>,
) -> Response {
    match edit_task_title_impl(&state, &user, TaskId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn edit_task_title_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    form: EditTaskTitleForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    let current = state.tasks.load(task_id).await?.ok_or(AppError::NotFound)?;
    apply_task_edit(
        state,
        user,
        task_id,
        role,
        &form.title,
        current.task.description.as_str(),
        "title",
    )
    .await
}

pub async fn edit_task_description_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<EditTaskDescriptionForm>,
) -> Response {
    match edit_task_description_impl(&state, &user, TaskId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn edit_task_description_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    form: EditTaskDescriptionForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    let current = state.tasks.load(task_id).await?.ok_or(AppError::NotFound)?;
    apply_task_edit(
        state,
        user,
        task_id,
        role,
        current.task.title.as_str(),
        &form.description,
        "description",
    )
    .await
}

/// The shared body of [`edit_task_title_impl`] and
/// [`edit_task_description_impl`]: both call `edit_task` with one field
/// changed and the other held at its current value, then handle the result
/// identically — re-rendering the task page either way, an inline
/// `422 error` re-render on a rule violation, or bubbling any other error up.
/// `open_hint` names which edit form the re-render should reopen on a rule
/// violation.
async fn apply_task_edit(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    role: Option<anamnesis_core::policy::Role>,
    title: &str,
    description: &str,
    open_hint: &'static str,
) -> Result<Response, WebError> {
    match edit_task(
        state.tasks.as_ref(),
        state.clock.as_ref(),
        state.search_index.as_ref(),
        role,
        task_id,
        title,
        description,
    )
    .await
    {
        Ok(task) => {
            // Re-indexed inside `edit_task` itself — see
            // `anamnesis_app::use_cases::indexing`'s module doc comment.
            render_task_page(
                state,
                user,
                task_id,
                &task,
                None,
                None,
                None,
                StatusCode::OK,
            )
            .await
        }
        Err(AppError::Rule(e)) => {
            let aggregate = view_task(state.tasks.as_ref(), role, task_id).await?;
            render_task_page(
                state,
                user,
                task_id,
                &aggregate.task,
                Some(&e.to_string()),
                Some(open_hint),
                None,
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await
        }
        Err(err) => Err(WebError::from(err)),
    }
}
