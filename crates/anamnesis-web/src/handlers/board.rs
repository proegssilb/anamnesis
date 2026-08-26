use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use uuid::Uuid;

use anamnesis_core::BoardId;

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::forms::CsrfOnlyForm;
use super::render::render_board_page;

pub async fn view_board_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(board_id): Path<Uuid>,
) -> Response {
    match view_board_impl(&state, &user, board_id).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn view_board_impl(
    state: &AppState,
    user: &CurrentUser,
    board_id: Uuid,
) -> Result<Response, WebError> {
    let board =
        anamnesis_app::view_board(state.repo.as_ref(), BoardId::new(board_id), &user.user_id)
            .await
            .map_err(WebError::from)?;
    render_board_page(state, user, &board, None, StatusCode::OK)
}

pub async fn delete_board_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(board_id): Path<Uuid>,
    Form(form): Form<CsrfOnlyForm>,
) -> Response {
    match delete_board_impl(&state, &user, board_id, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn delete_board_impl(
    state: &AppState,
    user: &CurrentUser,
    board_id: Uuid,
    form: CsrfOnlyForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    anamnesis_app::delete_board(state.repo.as_ref(), BoardId::new(board_id), &user.user_id)
        .await
        .map_err(WebError::from)?;
    Ok(Redirect::to("/boards").into_response())
}
