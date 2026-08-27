//! The legacy kanban application layer (`docs/ARCHITECTURE.md`, the
//! disposable scaffold described in `docs/DOMAIN.md`'s Context section).
//!
//! Preserved unchanged, save for `Clock`/`IdGen` moving to [`crate::ports`]
//! as shared infrastructure, purely so `anamnesis-web` keeps compiling
//! against it until Phase F removes both legacy layers
//! (`docs/DOMAIN.md` §7, §10). All new work lives in [`crate::ports`] and
//! [`crate::use_cases`], against the real domain model.

mod ports;
mod use_cases;

pub use ports::{
    Board, BoardRepository, BoardSummary, IdentityProvider, LoginCallback, LoginRedirect,
};
pub use use_cases::{
    add_card, add_column, create_board, delete_board, delete_card, edit_card, list_boards,
    move_card, view_board,
};
