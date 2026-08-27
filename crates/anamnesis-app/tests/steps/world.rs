//! The cucumber `World`: everything a scenario accumulates as it runs, plus
//! small helpers so step definitions read as behaviour, not bookkeeping.

use std::collections::HashMap;

use anamnesis_app::{AppError, Clock, IdGen};
use anamnesis_core::policy::Role;
use anamnesis_core::{
    AreaId, ColumnId, DetectedTangle, ProjectId, ProjectStatus, Reconciliation, Relationship,
    RelationshipId, Tangle, TangleId, Task, TaskId, TaskSummary, UserId,
};

use crate::domain_fakes::Fakes;
use crate::support::{FixedClock, SequentialIdGen};

#[derive(Debug, Default, cucumber::World)]
pub struct AppWorld {
    pub clock: FixedClock,
    pub ids: SequentialIdGen,
    users: HashMap<String, UserId>,

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

    // --- Phase D scenario state: access_control.feature / placement.feature.
    //
    // These exercise the real Phase D use cases (`anamnesis_app::use_cases`)
    // against the in-memory `domain_fakes::Fakes`, which implements every
    // port at once -- unlike the Phase B state above, this goes through the
    // actual application layer (role check, port, core transition) rather
    // than calling `anamnesis_core` directly.
    pub domain: Fakes,
    domain_projects: HashMap<String, ProjectId>,
    domain_tasks: HashMap<String, TaskId>,
    domain_columns: HashMap<String, ColumnId>,
    /// A named user's role, as most recently declared by a `Given` step.
    /// Absent (`None` from the map, distinct from `Some(None)`) means the
    /// scenario never mentioned a role for them, which
    /// `AppWorld::domain_role` treats identically to "no role assigned".
    domain_roles: HashMap<String, Option<Role>>,
    /// The most recent Phase D use-case result: `None` on success, the
    /// error otherwise. Distinct from `last_error` (the legacy Board
    /// scenarios' field) so the two suites cannot interfere.
    pub last_domain_error: Option<AppError>,
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

    // --- Phase D: access_control.feature / placement.feature helpers. ---

    /// Ensures a project named `project_name` exists (in its own,
    /// auto-created area), returning its id. Created directly against the
    /// fake store rather than through `create_project` -- the use case's own
    /// authorization is exercised by `domain_use_cases.rs` and by this
    /// suite's own access-control scenarios; a `Given` step here is scenario
    /// *setup*, not the behaviour under test.
    pub fn domain_project(&mut self, project_name: &str) -> ProjectId {
        if let Some(id) = self.domain_projects.get(project_name) {
            return *id;
        }
        let area_id = AreaId::new(self.ids.next());
        let area =
            anamnesis_core::create_area(area_id, project_name, "", 0, self.clock.now()).unwrap();
        self.domain.seed_area(area);
        let project_id = ProjectId::new(self.ids.next());
        let mut project =
            anamnesis_core::create_project(project_id, area_id, project_name, "", self.clock.now())
                .unwrap();
        project.status = ProjectStatus::Active;
        self.domain.seed_project(project);
        self.domain_projects
            .insert(project_name.to_string(), project_id);
        project_id
    }

    /// Ensures a project named `project_name` exists, returning the id of
    /// its (auto-created) Area -- the scope Area-role-inheritance scenarios
    /// assign roles on directly, via `Fakes::set_area_role`.
    pub fn domain_area_of(&mut self, project_name: &str) -> AreaId {
        let project_id = self.domain_project(project_name);
        self.domain.project(project_id).area_id
    }

    /// Creates a project named `project_name` inside an *existing* `area_id`
    /// -- unlike [`Self::domain_project`], which always mints a fresh Area
    /// of its own. Used to set up a sibling project within the same Area as
    /// another, already-created one.
    pub fn domain_project_in_area(&mut self, project_name: &str, area_id: AreaId) -> ProjectId {
        if let Some(id) = self.domain_projects.get(project_name) {
            return *id;
        }
        let project_id = ProjectId::new(self.ids.next());
        let mut project =
            anamnesis_core::create_project(project_id, area_id, project_name, "", self.clock.now())
                .unwrap();
        project.status = ProjectStatus::Active;
        self.domain.seed_project(project);
        self.domain_projects
            .insert(project_name.to_string(), project_id);
        project_id
    }

    /// Ensures a task named `task_name` exists (below the horizon) in
    /// `project_name`'s project, returning its id.
    pub fn domain_task(&mut self, task_name: &str, project_name: &str) -> TaskId {
        if let Some(id) = self.domain_tasks.get(task_name) {
            return *id;
        }
        let project_id = self.domain_project(project_name);
        let task_id = TaskId::new(self.ids.next());
        let task =
            anamnesis_core::create_task(task_id, project_id, task_name, "", self.clock.now())
                .unwrap();
        self.domain.seed_task(task);
        self.domain_tasks.insert(task_name.to_string(), task_id);
        task_id
    }

    pub fn domain_task_id(&self, task_name: &str) -> TaskId {
        *self
            .domain_tasks
            .get(task_name)
            .unwrap_or_else(|| panic!("scenario refers to unknown task {task_name:?}"))
    }

    /// The id of a column already established by a `Given` step. Panics if
    /// the scenario never mentioned it -- unlike [`Self::domain_column`],
    /// this is for `When`/`Then` steps that must refer to an *existing*
    /// column rather than create one with unspecified settings.
    pub fn domain_column_id(&self, column_name: &str) -> ColumnId {
        *self
            .domain_columns
            .get(column_name)
            .unwrap_or_else(|| panic!("scenario refers to unknown column {column_name:?}"))
    }

    /// Ensures a global board column named `column_name` exists, returning
    /// its id. `wip_limit` and `is_done` only take effect the first time a
    /// column of this name is mentioned.
    pub fn domain_column(
        &mut self,
        column_name: &str,
        wip_limit: Option<u32>,
        is_done: bool,
    ) -> ColumnId {
        if let Some(id) = self.domain_columns.get(column_name) {
            return *id;
        }
        let column_id = ColumnId::new(self.ids.next());
        let column =
            anamnesis_core::create_column(column_id, column_name, 0, wip_limit, is_done).unwrap();
        self.domain.seed_column(column);
        self.domain_columns
            .insert(column_name.to_string(), column_id);
        column_id
    }

    /// Reads a task straight out of the fake store, bypassing any use case.
    pub fn domain_task_state(&self, task_name: &str) -> Task {
        self.domain.task(self.domain_task_id(task_name))
    }

    /// Declares `user_name`'s role for the rest of the scenario.
    pub fn set_domain_role(&mut self, user_name: &str, role: Option<Role>) {
        self.domain_roles.insert(user_name.to_string(), role);
    }

    /// `user_name`'s most recently declared role, or `None` if the scenario
    /// never assigned one (an unmentioned user holds no role anywhere).
    pub fn domain_role(&self, user_name: &str) -> Option<Role> {
        self.domain_roles.get(user_name).copied().flatten()
    }
}
