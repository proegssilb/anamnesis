//! The original kanban `Board`/`Column`/`Card` domain model.
//!
//! This was always a disposable placeholder (see `docs/DOMAIN.md`), chosen to
//! prove the stack end to end. It has been replaced wholesale by the real
//! domain model at the crate root (`Area`, `Project`, `Task`, `Relationship`,
//! …). Nothing here is used by the new model.
//!
//! It is kept, unmodified in behaviour, purely so that `anamnesis-app`,
//! `anamnesis-adapters` and `anamnesis-web` — which are rebuilt against the
//! new model in later phases — keep compiling in the meantime. Do not add to
//! this module; new work belongs in the crate-root domain modules.

#![allow(deprecated)]

mod error;
mod model;
mod transitions;

#[deprecated(
    note = "placeholder kanban model, superseded by docs/DOMAIN.md; see `legacy` docs"
)]
pub use error::DomainError;
#[deprecated(
    note = "placeholder kanban model, superseded by docs/DOMAIN.md; see `legacy` docs"
)]
pub use model::{Board, Card, Column};
#[deprecated(
    note = "placeholder kanban model, superseded by docs/DOMAIN.md; see `legacy` docs"
)]
pub use transitions::{
    add_card, add_column, can_view, create_board, edit_card, move_card, remove_card, remove_column,
    rename_column,
};
