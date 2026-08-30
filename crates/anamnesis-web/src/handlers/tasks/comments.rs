use axum::Form;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};

use anamnesis_app::add_comment;
use anamnesis_core::TaskId;

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use crate::handlers::forms::AddCommentForm;

use super::role_for_task;

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
