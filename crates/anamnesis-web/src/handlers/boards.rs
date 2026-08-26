use axum::Form;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

use anamnesis_app::AppError;

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::forms::CreateBoardForm;
use super::render::render_boards_page;

pub async fn list_boards_handler(State(state): State<AppState>, user: CurrentUser) -> Response {
    match list_boards_impl(&state, &user).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn list_boards_impl(state: &AppState, user: &CurrentUser) -> Result<Response, WebError> {
    let boards = anamnesis_app::list_boards(state.repo.as_ref(), &user.user_id)
        .await
        .map_err(WebError::from)?;
    render_boards_page(state, user, &boards, None, StatusCode::OK)
}

pub async fn create_board_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Form(form): Form<CreateBoardForm>,
) -> Response {
    match create_board_impl(&state, &user, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn create_board_impl(
    state: &AppState,
    user: &CurrentUser,
    form: CreateBoardForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }

    match anamnesis_app::create_board(
        state.repo.as_ref(),
        state.id_gen.as_ref(),
        &user.user_id,
        &form.title,
    )
    .await
    {
        Ok(board) => Ok(Redirect::to(&format!("/boards/{}", board.id)).into_response()),
        Err(AppError::Domain(domain_err)) => {
            let boards = anamnesis_app::list_boards(state.repo.as_ref(), &user.user_id)
                .await
                .map_err(WebError::from)?;
            render_boards_page(
                state,
                user,
                &boards,
                Some(&domain_err.to_string()),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
        }
        Err(err) => Err(WebError::from(err)),
    }
}
