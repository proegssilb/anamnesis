//! The `Board` aggregate: `Board` owns its `Column`s, each `Column` owns its
//! ordered `Card`s. `Vec` order is canonical card/column order in the core;
//! persisted `position` integers are an adapter concern derived from it.
//!
//! These are plain data carriers with no invariants of their own — they hold
//! whatever a caller puts in them. The rules live in `transitions`: the only
//! supported way to build or change a `Board` is through those pure
//! functions, which is what keeps every `Board` you can observe valid. There
//! is no behaviour here to unit-test in isolation; every field and every
//! piece of `Vec` ordering is exercised by the transition tests.

use serde::{Deserialize, Serialize};

use crate::ids::{BoardId, CardId, ColumnId, Timestamp, UserId};
use crate::title::Title;

/// A card: a single unit of work sitting in a column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub id: CardId,
    pub title: Title,
    pub body: String,
    pub created_at: Timestamp,
}

/// A column: an ordered list of cards, optionally capped by a WIP limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub id: ColumnId,
    pub title: Title,
    pub wip_limit: Option<u16>,
    pub cards: Vec<Card>,
}

/// A board: the aggregate root and single consistency boundary. Owned by
/// exactly one user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    pub id: BoardId,
    pub owner: UserId,
    pub title: Title,
    pub columns: Vec<Column>,
}
