//! Shared page-rendering helpers. Both the happy-path `GET` handlers and the
//! `422`-with-the-board-re-rendered error path (see `docs/PLAN.md`'s
//! `AppError -> status` table) go through these, so the two can never drift
//! apart in what context a page expects.

use anamnesis_app::{Board, BoardSummary};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use minijinja::context;

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::state::AppState;

pub fn render_board_page(
    state: &AppState,
    user: &CurrentUser,
    board: &Board,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, WebError> {
    let tmpl = state
        .templates
        .get_template("board.html")
        .map_err(WebError::template)?;
    let body = tmpl
        .render(context! {
            board => board,
            csrf_token => user.csrf_token,
            current_user => user.display_name,
            error => error,
        })
        .map_err(WebError::template)?;
    Ok((status, Html(body)).into_response())
}

pub fn render_boards_page(
    state: &AppState,
    user: &CurrentUser,
    boards: &[BoardSummary],
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, WebError> {
    let tmpl = state
        .templates
        .get_template("boards.html")
        .map_err(WebError::template)?;
    let body = tmpl
        .render(context! {
            boards => boards,
            csrf_token => user.csrf_token,
            current_user => user.display_name,
            error => error,
        })
        .map_err(WebError::template)?;
    Ok((status, Html(body)).into_response())
}
