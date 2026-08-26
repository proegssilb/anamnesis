#![forbid(unsafe_code)]
//! `anamnesis-app`: the application layer. Declares the port traits the use
//! cases need from the world, owns `AppError`, and implements the use cases
//! themselves as boring orchestration — load an aggregate through a port,
//! call one pure `anamnesis-core` transition, save the result through a
//! port, map errors. See `docs/ARCHITECTURE.md`.

mod error;
mod ports;
mod use_cases;

pub use error::{AppError, IdentityError, RepoError};
pub use ports::{
    Board, BoardRepository, BoardSummary, Clock, IdGen, IdentityProvider, LoginCallback,
    LoginRedirect,
};
pub use use_cases::{
    add_card, add_column, create_board, delete_board, delete_card, edit_card, list_boards,
    move_card, view_board,
};
