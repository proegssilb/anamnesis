//! Request handlers, one small file per resource. Every mutating handler
//! follows the same shape: a thin `pub async fn ..._handler` axum can route
//! to, delegating to a private `..._impl` that returns
//! `Result<Response, WebError>` so it can use `?`; the thin wrapper turns an
//! `Err` into a rendered error page via [`WebError::into_response_with`].

mod board;
mod boards;
mod cards;
mod columns;
mod forms;
mod login;
mod misc;
mod render;

pub use board::{delete_board_handler, view_board_handler};
pub use boards::{create_board_handler, list_boards_handler};
pub use cards::{add_card_handler, delete_card_handler, move_card_handler};
pub use columns::add_column_handler;
pub use login::{callback_handler, login_handler, logout_handler};
pub use misc::{healthz_handler, root_handler};
