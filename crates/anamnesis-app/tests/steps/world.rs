//! The cucumber `World`: everything a scenario accumulates as it runs, plus
//! small helpers so step definitions read as behaviour, not bookkeeping.

use std::collections::HashMap;

use anamnesis_app::{AppError, Board, BoardRepository};
use anamnesis_core::{BoardId, CardId, ColumnId, UserId};

use crate::support::{FixedClock, InMemoryBoardRepository, SequentialIdGen};

#[derive(Debug, Default, cucumber::World)]
pub struct AppWorld {
    pub repo: InMemoryBoardRepository,
    pub clock: FixedClock,
    pub ids: SequentialIdGen,
    users: HashMap<String, UserId>,
    boards: HashMap<String, (BoardId, UserId)>,
    columns: HashMap<(String, String), ColumnId>,
    cards: HashMap<(String, String), CardId>,
    /// The board returned by the most recent use-case call, whether it
    /// succeeded or a Given step set it up.
    pub last_board: Option<Board>,
    /// The error returned by the most recent use-case call, if any.
    pub last_error: Option<AppError>,
}

impl AppWorld {
    /// Returns the `UserId` for `name`, registering it the first time it is
    /// mentioned. There is no separate "sign up" step: a name mentioned in a
    /// scenario is a user that exists.
    pub fn user(&mut self, name: &str) -> UserId {
        self.users
            .entry(name.to_string())
            .or_insert_with(|| UserId::new(name))
            .clone()
    }

    /// Ensures a board named `board_name`, owned by `owner_name`, exists —
    /// creating it the first time it is mentioned so scenarios can add
    /// several columns to "the same" board across multiple Given lines.
    pub async fn ensure_board(&mut self, owner_name: &str, board_name: &str) -> BoardId {
        if let Some((id, _)) = self.boards.get(board_name) {
            return *id;
        }
        let owner = self.user(owner_name);
        let board = anamnesis_app::create_board(&self.repo, &self.ids, &owner, board_name)
            .await
            .expect("scenario setup: create_board must succeed");
        self.boards
            .insert(board_name.to_string(), (board.id, owner));
        self.last_board = Some(board.clone());
        board.id
    }

    pub fn board_id(&self, board_name: &str) -> BoardId {
        self.boards
            .get(board_name)
            .unwrap_or_else(|| panic!("scenario refers to unknown board {board_name:?}"))
            .0
    }

    pub fn board_owner(&self, board_name: &str) -> UserId {
        self.boards
            .get(board_name)
            .unwrap_or_else(|| panic!("scenario refers to unknown board {board_name:?}"))
            .1
            .clone()
    }

    pub fn remember_column(&mut self, board_name: &str, title: &str, id: ColumnId) {
        self.columns
            .insert((board_name.to_string(), title.to_string()), id);
    }

    pub fn column_id(&self, board_name: &str, title: &str) -> ColumnId {
        *self
            .columns
            .get(&(board_name.to_string(), title.to_string()))
            .unwrap_or_else(|| panic!("scenario refers to unknown column {title:?}"))
    }

    pub fn remember_card(&mut self, board_name: &str, title: &str, id: CardId) {
        self.cards
            .insert((board_name.to_string(), title.to_string()), id);
    }

    pub fn card_id(&self, board_name: &str, title: &str) -> CardId {
        *self
            .cards
            .get(&(board_name.to_string(), title.to_string()))
            .unwrap_or_else(|| panic!("scenario refers to unknown card {title:?}"))
    }

    /// Loads a board straight from the repository, bypassing any use case —
    /// what `Then` steps assert against, independent of what the most
    /// recent action happened to return.
    pub async fn reload(&self, board_name: &str) -> Board {
        self.repo
            .load(self.board_id(board_name))
            .await
            .unwrap()
            .expect("board vanished from the repository")
    }
}
