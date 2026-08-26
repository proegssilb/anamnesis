#![forbid(unsafe_code)]

mod error;
mod ids;
mod model;
mod title;
mod transitions;

pub use error::DomainError;
pub use ids::{BoardId, CardId, ColumnId, Timestamp, TimestampError, UserId};
pub use model::{Board, Card, Column};
pub use title::{Title, TitleError};
pub use transitions::{
    add_card, add_column, can_view, create_board, edit_card, move_card, remove_card, remove_column,
    rename_column,
};
