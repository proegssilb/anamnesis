use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use uuid::Uuid;

use anamnesis_app::AppError;
use anamnesis_core::BoardId;

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::forms::AddColumnForm;
use super::render::render_board_page;

pub async fn add_column_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(board_id): Path<Uuid>,
    Form(form): Form<AddColumnForm>,
) -> Response {
    match add_column_impl(&state, &user, board_id, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn add_column_impl(
    state: &AppState,
    user: &CurrentUser,
    board_id: Uuid,
    form: AddColumnForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }

    match anamnesis_app::add_column(
        state.repo.as_ref(),
        state.id_gen.as_ref(),
        BoardId::new(board_id),
        &user.user_id,
        &form.title,
        form.wip_limit,
    )
    .await
    {
        Ok(board) => {
            // `add_column` always appends, so the new column is the last one.
            let new_column_id = board.columns.last().expect("a column was just added").id;
            Ok(Redirect::to(&format!("/boards/{board_id}#column-{new_column_id}")).into_response())
        }
        Err(AppError::Domain(domain_err)) => {
            let board = anamnesis_app::view_board(
                state.repo.as_ref(),
                BoardId::new(board_id),
                &user.user_id,
            )
            .await
            .map_err(WebError::from)?;
            render_board_page(
                state,
                user,
                &board,
                Some(&domain_err.to_string()),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
        }
        Err(err) => Err(WebError::from(err)),
    }
}
