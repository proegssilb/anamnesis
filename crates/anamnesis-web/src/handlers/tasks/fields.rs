use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

use anamnesis_app::{AppError, set_task_field_value, view_task};
use anamnesis_core::{FieldId, TaskId};

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use crate::handlers::field_form;
use crate::handlers::forms::SetFieldValueForm;

use super::page::render_task_page;
use super::role_for_task;

/// Sets a task's value for one of its project's custom fields
/// (`docs/DOMAIN.md` §3) — the form every [`anamnesis_core::FieldKind`]
/// needed and never had before this phase (see `crate::handlers::field_form`'s
/// module doc comment).
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
            return render_field_value_error(state, user, role, task_id, &message).await;
        }
        Err(err) => return Err(err),
    };

    match set_task_field_value(state.tasks.as_ref(), role, definition, task_id, data).await {
        Ok(_) => Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response()),
        Err(AppError::Rule(e)) => {
            render_field_value_error(state, user, role, task_id, &e.to_string()).await
        }
        Err(err) => Err(WebError::from(err)),
    }
}

/// Re-renders the task page with a `422` and `message` set as the inline
/// error — the shared tail of [`set_field_value_impl`]'s two failure paths
/// (a field value that fails to parse, and one that parses but violates a
/// domain rule on save).
async fn render_field_value_error(
    state: &AppState,
    user: &CurrentUser,
    role: Option<anamnesis_core::policy::Role>,
    task_id: TaskId,
    message: &str,
) -> Result<Response, WebError> {
    let aggregate = view_task(state.tasks.as_ref(), role, task_id).await?;
    render_task_page(
        state,
        user,
        task_id,
        &aggregate.task,
        Some(message),
        None,
        None,
        StatusCode::UNPROCESSABLE_ENTITY,
    )
    .await
}
