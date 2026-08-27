//! The cucumber `World`: everything a scenario accumulates as it runs, plus
//! small helpers so step definitions read as behaviour, not bookkeeping.

use std::collections::HashMap;

use anamnesis_app::{AppError, Board, BoardRepository, Clock, IdGen};
use anamnesis_core::{
    BoardId, CardId, ColumnId, DetectedTangle, ProjectId, ProjectStatus, Reconciliation,
    Relationship, RelationshipId, Tangle, TangleId, TaskId, TaskSummary, UserId,
};

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

    // --- Phase B scenario state: tangles.feature / suggestions.feature.
    //
    // These scenarios exercise `anamnesis_core::tangle`/`suggest` directly —
    // pure functions, no repository, no use case — so this state lives
    // entirely in the `World` rather than behind a port.
    /// Named tasks, by scenario name (e.g. "A"), all sharing one synthetic
    /// project — the project boundary is irrelevant to tangle detection or
    /// the suggestion engine's per-candidate fields.
    core_tasks: HashMap<String, TaskId>,
    /// `blocks` edges declared so far, by scenario task name.
    pub relationships: Vec<Relationship>,
    /// The raw result of the most recent `detect_tangles` call.
    pub last_detected: Vec<DetectedTangle>,
    /// Tangles treated as "already stored" going into the next detection
    /// pass — what a real system would have persisted from a previous
    /// `reconcile` call.
    pub stored_tangles: Vec<Tangle>,
    /// The result of the most recent `reconcile` call.
    pub last_reconciliation: Option<Reconciliation>,
    /// Suggestion-engine candidates, by scenario task name.
    pub task_summaries: HashMap<String, TaskSummary>,
    /// The board state the most recent `suggest` call used.
    pub board_state: Option<anamnesis_core::BoardState>,
    /// The result of the most recent `suggest` call.
    pub last_outcome: Option<anamnesis_core::Outcome>,
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

    // --- Phase B: tangles.feature / suggestions.feature helpers. ---

    /// The one synthetic project every scenario task in these two features
    /// belongs to — an arbitrary fixed id, since neither `detect_tangles`
    /// nor `suggest`'s own logic cares which project a task is in beyond its
    /// `ProjectStatus`.
    pub fn core_project_id(&self) -> ProjectId {
        ProjectId::new(uuid::Uuid::from_u128(0xC0DE))
    }

    /// Returns the `TaskId` for a scenario task name, registering it (with a
    /// fresh id) the first time it is mentioned — the same "first mention
    /// creates it" convention as [`AppWorld::user`].
    pub fn core_task(&mut self, name: &str) -> TaskId {
        if let Some(id) = self.core_tasks.get(name) {
            return *id;
        }
        let id = TaskId::new(self.ids.next());
        self.core_tasks.insert(name.to_string(), id);
        id
    }

    /// Records a `blocks` edge from `from` to `to` (both scenario task
    /// names), creating either task if this is its first mention.
    pub fn add_blocks_edge(&mut self, from: &str, to: &str) {
        let from_id = self.core_task(from);
        let to_id = self.core_task(to);
        let project = self.core_project_id();
        let relationship = anamnesis_core::create_relationship(
            RelationshipId::new(self.ids.next()),
            from_id,
            project,
            to_id,
            project,
            &anamnesis_core::builtin_blocks(),
            self.clock.now(),
        )
        .expect("scenario setup: a blocks edge between two distinct tasks must be valid");
        self.relationships.push(relationship);
    }

    /// Removes the `blocks` edge from `from` to `to`, panicking if no such
    /// edge was ever declared — a scenario that references breaking a block
    /// that was never there is a scenario bug, not a system behaviour.
    pub fn remove_blocks_edge(&mut self, from: &str, to: &str) {
        let from_id = self.core_task(from);
        let to_id = self.core_task(to);
        let before = self.relationships.len();
        self.relationships
            .retain(|r| !(r.from_task_id == from_id && r.to_task_id == to_id));
        assert_eq!(
            self.relationships.len(),
            before - 1,
            "scenario removed a blocks edge ({from:?} -> {to:?}) that was never declared"
        );
    }

    /// Registers (or updates) the suggestion-engine candidate summary for a
    /// scenario task name.
    pub fn set_task_summary(
        &mut self,
        name: &str,
        placement: anamnesis_core::Placement,
        project_status: ProjectStatus,
    ) {
        let task_id = self.core_task(name);
        self.task_summaries.insert(
            name.to_string(),
            TaskSummary {
                task_id,
                archived: false,
                placement,
                project_status,
                last_touched_at: self.clock.now(),
                last_offered_at: None,
                bounce_count: 0,
            },
        );
    }

    /// A fresh `TangleId`, for scenario steps that need to hand one to
    /// `reconcile`.
    pub fn fresh_tangle_id(&self) -> TangleId {
        TangleId::new(self.ids.next())
    }
}
