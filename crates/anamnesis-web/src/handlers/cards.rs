use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use uuid::Uuid;

use anamnesis_app::AppError;
use anamnesis_core::{BoardId, CardId, ColumnId};

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::forms::{AddCardForm, CsrfOnlyForm, MoveCardForm};
use super::render::render_board_page;

/// Re-fetches `board_id` and re-renders it with `error`, for the `422` path
/// every card mutation shares: the mutation failed a domain rule, so the
/// board itself is unchanged and just needs to be shown again with the
/// reason.
async fn rerender_with_domain_error(
    state: &AppState,
    user: &CurrentUser,
    board_id: Uuid,
    message: &str,
) -> Result<Response, WebError> {
    let board =
        anamnesis_app::view_board(state.repo.as_ref(), BoardId::new(board_id), &user.user_id)
            .await
            .map_err(WebError::from)?;
    render_board_page(
        state,
        user,
        &board,
        Some(message),
        StatusCode::UNPROCESSABLE_ENTITY,
    )
}

pub async fn add_card_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((board_id, column_id)): Path<(Uuid, Uuid)>,
    Form(form): Form<AddCardForm>,
) -> Response {
    match add_card_impl(&state, &user, board_id, column_id, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn add_card_impl(
    state: &AppState,
    user: &CurrentUser,
    board_id: Uuid,
    column_id: Uuid,
    form: AddCardForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }

    match anamnesis_app::add_card(
        state.repo.as_ref(),
        state.id_gen.as_ref(),
        state.clock.as_ref(),
        BoardId::new(board_id),
        &user.user_id,
        ColumnId::new(column_id),
        &form.title,
        &form.body,
    )
    .await
    {
        Ok(board) => {
            let new_card_id = board
                .columns
                .iter()
                .find(|c| c.id == ColumnId::new(column_id))
                .and_then(|c| c.cards.last())
                .expect("a card was just added to this column")
                .id;
            Ok(Redirect::to(&format!("/boards/{board_id}#card-{new_card_id}")).into_response())
        }
        Err(AppError::Domain(domain_err)) => {
            rerender_with_domain_error(state, user, board_id, &domain_err.to_string()).await
        }
        Err(err) => Err(WebError::from(err)),
    }
}

pub async fn move_card_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((board_id, card_id)): Path<(Uuid, Uuid)>,
    Form(form): Form<MoveCardForm>,
) -> Response {
    match move_card_impl(&state, &user, board_id, card_id, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn move_card_impl(
    state: &AppState,
    user: &CurrentUser,
    board_id: Uuid,
    card_id: Uuid,
    form: MoveCardForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }

    match anamnesis_app::move_card(
        state.repo.as_ref(),
        BoardId::new(board_id),
        &user.user_id,
        CardId::new(card_id),
        ColumnId::new(form.to_column),
        form.to_index,
    )
    .await
    {
        Ok(_board) => {
            Ok(Redirect::to(&format!("/boards/{board_id}#card-{card_id}")).into_response())
        }
        Err(AppError::Domain(domain_err)) => {
            rerender_with_domain_error(state, user, board_id, &domain_err.to_string()).await
        }
        Err(err) => Err(WebError::from(err)),
    }
}

pub async fn delete_card_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((board_id, card_id)): Path<(Uuid, Uuid)>,
    Form(form): Form<CsrfOnlyForm>,
) -> Response {
    match delete_card_impl(&state, &user, board_id, card_id, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn delete_card_impl(
    state: &AppState,
    user: &CurrentUser,
    board_id: Uuid,
    card_id: Uuid,
    form: CsrfOnlyForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }

    match anamnesis_app::delete_card(
        state.repo.as_ref(),
        BoardId::new(board_id),
        &user.user_id,
        CardId::new(card_id),
    )
    .await
    {
        Ok(_board) => {
            Ok(Redirect::to(&format!("/boards/{board_id}#card-{card_id}")).into_response())
        }
        Err(AppError::Domain(domain_err)) => {
            rerender_with_domain_error(state, user, board_id, &domain_err.to_string()).await
        }
        Err(err) => Err(WebError::from(err)),
    }
}
