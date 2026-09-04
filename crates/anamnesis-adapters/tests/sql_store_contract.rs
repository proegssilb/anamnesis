//! The `SqlStore` contract for the real domain model (`docs/DOMAIN.md`),
//! exercised once and run against both backends so they cannot drift —
//! exactly as `tests/board_repository.rs` does for the legacy model.
//!
//! SQLite runs against a temporary **file** (not `:memory:` — a pool opens
//! multiple connections, and each would otherwise get its own empty
//! in-memory database). Postgres runs when `ANAMNESIS_TEST_PG_URL` is set;
//! it is `#[ignore]`d otherwise so `cargo test` stays green without a live
//! Postgres server.

use std::collections::BTreeSet;
use std::time::Duration;

use anamnesis_adapters::SqlStore;
use anamnesis_app::{
    AreaRepository, Attachment, AttachmentId, AttachmentKind, AttachmentRepository, BoardQuery,
    Comment, CommentId, CommentRepository, JobLease, MembershipQuery, MembershipRepository,
    ProjectAggregate, ProjectRepository, RelationshipRepository, SearchHit, SearchIndex,
    SearchQuery, Settings, SettingsRepository, TangleRepository, TaskAggregate, TaskRepository,
    TaskUpdateError,
};
use anamnesis_core::policy::Role;
use anamnesis_core::{
    Area, BlockingGraph, Column, CurrencyAmount, CurrencyCode, FieldData, FieldDefinition,
    FieldKind, FieldValue, KindId, NumberValue, Placement, Project, ProjectId, ProjectStatus,
    Recurrence, Relationship, RelationshipKind, SuggestionSettings, Tangle, TangleId, Task, TaskId,
    Timestamp, Title, UserId,
};
use uuid::Uuid;

fn ts(secs: i64) -> Timestamp {
    Timestamp::from_unix_seconds(secs).unwrap()
}

fn title(raw: &str) -> Title {
    Title::new(raw).unwrap()
}

fn area(position: u32) -> Area {
    Area {
        id: Uuid::new_v4().into(),
        title: title("Home"),
        description: "household stuff".to_string(),
        position,
        created_at: ts(1_000),
        updated_at: ts(1_000),
    }
}

fn project(area_id: anamnesis_core::AreaId, status: ProjectStatus) -> Project {
    Project {
        id: Uuid::new_v4().into(),
        area_id,
        title: title("Kitchen remodel"),
        description: "".to_string(),
        status,
        created_at: ts(1_000),
        updated_at: ts(1_000),
        archived_at: None,
    }
}

fn field_def(project_id: ProjectId, kind: FieldKind, position: u32, name: &str) -> FieldDefinition {
    FieldDefinition {
        id: Uuid::new_v4().into(),
        project_id,
        name: title(name),
        kind,
        position,
        show_on_card: true,
    }
}

fn task(project_id: ProjectId) -> Task {
    Task {
        id: Uuid::new_v4().into(),
        project_id,
        title: title("Regrout the shower"),
        description: "".to_string(),
        placement: Placement::Below,
        parent_task_id: None,
        checklist_position: 0,
        created_at: ts(2_000),
        last_touched_at: ts(2_000),
        archived_at: None,
        bounce_count: 0,
        last_bounced_at: None,
        last_offered_at: None,
    }
}

fn column(position: u32, wip_limit: Option<u32>, is_done: bool) -> Column {
    Column {
        id: Uuid::new_v4().into(),
        title: title(if is_done { "Done" } else { "To-Do" }),
        position,
        wip_limit,
        is_done,
    }
}

/// Shared arrange step repeated by nearly every contract below: an area,
/// and a project inside it, both already persisted.
async fn seed_area_and_project(
    store: &SqlStore,
    area_position: u32,
    status: ProjectStatus,
) -> (Area, Project) {
    let owning_area = area(area_position);
    AreaRepository::insert(store, &owning_area).await.unwrap();
    let p = project(owning_area.id, status);
    ProjectRepository::insert(store, &p).await.unwrap();
    (owning_area, p)
}

/// The shared contract. Called once per backend so the two implementations
/// cannot drift apart.
async fn contract(store: &SqlStore) {
    area_contract(store).await;
    let created_project = project_contract(store).await;
    project_relationship_kind_contract(store, created_project.id).await;
    project_active_limit_contract(store, created_project).await;
    let (task_contract_project, task_a, task_b) = task_contract(store).await;
    field_value_contract(store, task_contract_project, task_a.id).await;
    optimistic_concurrency_contract(store, &task_a).await;
    relationship_contract(store, &task_a, &task_b).await;
    tangle_contract(store).await;
    board_and_suggestion_contract(store, task_contract_project).await;
    tangle_on_board_contract(store, task_contract_project).await;
    comment_contract(store, &task_a).await;
    attachment_contract(store, &task_a).await;
    membership_contract(store).await;
    search_contract(store).await;
    settings_contract(store).await;
    job_lease_contract(store).await;
}

// --- Area ---

async fn area_contract(store: &SqlStore) {
    let missing = AreaRepository::load(store, Uuid::new_v4().into())
        .await
        .unwrap();
    assert_eq!(
        missing, None,
        "loading an unsaved area is None, not an error"
    );

    let a = area(5);
    let b = area(1);
    AreaRepository::insert(store, &a).await.unwrap();
    AreaRepository::insert(store, &b).await.unwrap();

    let loaded = AreaRepository::load(store, a.id).await.unwrap().unwrap();
    assert_eq!(loaded, a, "round-tripped area must equal the saved area");

    let listed = AreaRepository::list(store).await.unwrap();
    let positions: Vec<u32> = listed
        .iter()
        .filter(|x| x.id == a.id || x.id == b.id)
        .map(|x| x.position)
        .collect();
    assert_eq!(
        positions,
        vec![1, 5],
        "list() must be ordered by position, surviving the round trip"
    );

    let mut edited = a.clone();
    edited.title = title("Home (renamed)");
    edited.description = "new description".to_string();
    edited.position = 9;
    edited.updated_at = ts(3_000);
    AreaRepository::update(store, &edited).await.unwrap();
    let reloaded = AreaRepository::load(store, a.id).await.unwrap().unwrap();
    assert_eq!(reloaded, edited, "update must be a targeted, visible write");
}

// --- Project (+ field definitions + relationship kinds) ---

async fn project_contract(store: &SqlStore) -> Project {
    let (owning_area, p) = seed_area_and_project(store, 0, ProjectStatus::Pending).await;

    let loaded = ProjectRepository::load(store, p.id).await.unwrap().unwrap();
    assert_eq!(
        loaded,
        ProjectAggregate {
            project: p.clone(),
            field_definitions: Vec::new(),
            relationship_kinds: Vec::new(),
        },
        "a freshly inserted project has no fields or kinds yet"
    );

    let second = field_def(p.id, FieldKind::Line, 1, "Notes");
    let first = field_def(p.id, FieldKind::Number, 0, "Weight");
    // Insert out of position order to prove ordering comes from the stored
    // `position` column, not insertion order.
    ProjectRepository::insert_field_definition(store, &second)
        .await
        .unwrap();
    ProjectRepository::insert_field_definition(store, &first)
        .await
        .unwrap();

    let loaded = ProjectRepository::load(store, p.id).await.unwrap().unwrap();
    assert_eq!(
        loaded.field_definitions,
        vec![first.clone(), second.clone()],
        "field definitions load ordered by position, not insertion order"
    );

    let renamed = anamnesis_core::rename_field_definition(&first, "Weight (kg)").unwrap();
    ProjectRepository::update_field_definition(store, &renamed)
        .await
        .unwrap();
    let loaded = ProjectRepository::load(store, p.id).await.unwrap().unwrap();
    assert_eq!(loaded.field_definitions[0].name.as_str(), "Weight (kg)");

    let by_area = ProjectRepository::list_by_area(store, owning_area.id)
        .await
        .unwrap();
    assert_eq!(by_area, vec![p.clone()]);

    p
}

// --- Project: count_active / active-project-limit support ---

async fn project_active_limit_contract(store: &SqlStore, p: Project) {
    assert_eq!(
        ProjectRepository::count_active(store, None).await.unwrap(),
        0
    );
    let mut active = p.clone();
    active.status = ProjectStatus::Active;
    ProjectRepository::update(store, &active).await.unwrap();
    assert_eq!(
        ProjectRepository::count_active(store, None).await.unwrap(),
        1
    );
    assert_eq!(
        ProjectRepository::count_active(store, Some(p.id))
            .await
            .unwrap(),
        0,
        "excluding the only active project must count to zero"
    );

    let loaded = ProjectRepository::load(store, p.id).await.unwrap().unwrap();
    assert_eq!(loaded.project.status, ProjectStatus::Active);
}

// --- Project: relationship kinds ---

async fn project_relationship_kind_contract(store: &SqlStore, project_id: ProjectId) {
    let kind = RelationshipKind {
        id: Uuid::new_v4().into(),
        project_id: Some(project_id),
        forward_label: title("inspired by"),
        reverse_label: title("inspired"),
    };
    ProjectRepository::insert_relationship_kind(store, &kind)
        .await
        .unwrap();

    let loaded = ProjectRepository::load(store, project_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.relationship_kinds, vec![kind.clone()]);

    let loaded_kind = ProjectRepository::load_relationship_kind(store, kind.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded_kind, kind);
    assert_eq!(
        ProjectRepository::load_relationship_kind(store, KindId::BUILTIN_BLOCKS)
            .await
            .unwrap(),
        None,
        "a builtin kind id is never a stored row"
    );
}

// --- Task (+ field values, typed EAV) + list_children ordering ---

async fn task_contract(store: &SqlStore) -> (ProjectId, Task, Task) {
    let (_, p) = seed_area_and_project(store, 2, ProjectStatus::Active).await;

    let missing = TaskRepository::load(store, Uuid::new_v4().into())
        .await
        .unwrap();
    assert_eq!(missing, None);

    let a = task(p.id);
    let b = task(p.id);
    TaskRepository::insert(store, &a).await.unwrap();
    TaskRepository::insert(store, &b).await.unwrap();

    let loaded = TaskRepository::load(store, a.id).await.unwrap().unwrap();
    assert_eq!(
        loaded,
        TaskAggregate {
            task: a.clone(),
            field_values: Vec::new(),
        }
    );

    task_listing_and_archive_contract(store, p.id, &a, &b).await;

    (p.id, a, b)
}

/// Checklist ordering and `list_by_project`/`list_children`'s exclusion of
/// archived tasks -- a distinct concern from `task_contract`'s basic
/// insert/load round trip above.
async fn task_listing_and_archive_contract(
    store: &SqlStore,
    project_id: ProjectId,
    a: &Task,
    b: &Task,
) {
    // Checklist containment + ordering: two children, inserted out of
    // order, must list back ordered by checklist_position.
    let (child_a, child_b) = seed_checklist_children(store, project_id, a.id).await;

    let children = TaskRepository::list_children(store, a.id).await.unwrap();
    assert_eq!(
        children.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![child_b.id, child_a.id],
        "children must list ordered by checklist_position, not insertion order"
    );

    task_archive_exclusion_contract(store, project_id, a, b, &child_a, &child_b).await;
}

async fn seed_checklist_children(
    store: &SqlStore,
    project_id: ProjectId,
    parent_id: TaskId,
) -> (Task, Task) {
    let mut child_a = task(project_id);
    child_a.parent_task_id = Some(parent_id);
    child_a.checklist_position = 1;
    let mut child_b = task(project_id);
    child_b.parent_task_id = Some(parent_id);
    child_b.checklist_position = 0;
    TaskRepository::insert(store, &child_a).await.unwrap();
    TaskRepository::insert(store, &child_b).await.unwrap();
    (child_a, child_b)
}

/// `list_by_project`: every non-archived task in the project, regardless of
/// placement or checklist depth — the "project as a flat list" query. Both
/// it and `list_children` must exclude archived tasks; an archived
/// checklist item must not keep appearing in its parent's checklist
/// (mirrors the `list_by_area` fix for archived projects, Phase F3).
async fn task_archive_exclusion_contract(
    store: &SqlStore,
    project_id: ProjectId,
    a: &Task,
    b: &Task,
    child_a: &Task,
    child_b: &Task,
) {
    let mut in_project = TaskRepository::list_by_project(store, project_id)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect::<Vec<_>>();
    in_project.sort_by_key(|id| id.as_uuid());
    let mut expected = vec![a.id, b.id, child_a.id, child_b.id];
    expected.sort_by_key(|id| id.as_uuid());
    assert_eq!(
        in_project, expected,
        "list_by_project must return every task in the project, including checklist children"
    );

    let archived = anamnesis_core::archive_task(b, ts(999)).unwrap();
    TaskRepository::update(store, &archived, b.last_touched_at)
        .await
        .unwrap();
    let after_archive = TaskRepository::list_by_project(store, project_id)
        .await
        .unwrap();
    assert!(
        !after_archive.iter().any(|t| t.id == b.id),
        "list_by_project must exclude archived tasks"
    );

    let archived_child = anamnesis_core::archive_task(child_a, ts(998)).unwrap();
    TaskRepository::update(store, &archived_child, child_a.last_touched_at)
        .await
        .unwrap();
    let children_after_archive = TaskRepository::list_children(store, a.id).await.unwrap();
    assert_eq!(
        children_after_archive
            .iter()
            .map(|t| t.id)
            .collect::<Vec<_>>(),
        vec![child_b.id],
        "list_children must exclude archived children"
    );
}

// --- Task field values: typed EAV round trip + overwrite semantics ---

/// One field definition per `FieldKind`, inserted on `project_id`, to
/// round-trip every variant of the typed EAV encoding.
async fn seed_one_field_definition_per_kind(
    store: &SqlStore,
    project_id: ProjectId,
) -> [FieldDefinition; 7] {
    let defs = [
        field_def(project_id, FieldKind::Number, 0, "Weight"),
        field_def(project_id, FieldKind::Currency, 1, "Cost"),
        field_def(project_id, FieldKind::Date, 2, "Due"),
        field_def(project_id, FieldKind::Time, 3, "Reminder"),
        field_def(project_id, FieldKind::DateTime, 4, "Scheduled"),
        field_def(project_id, FieldKind::Line, 5, "Summary"),
        field_def(project_id, FieldKind::Block, 6, "Notes"),
    ];
    for def in &defs {
        ProjectRepository::insert_field_definition(store, def)
            .await
            .unwrap();
    }
    defs
}

/// One instance of every `FieldData` variant, targeting `task_id` on the
/// matching definition from [`seed_one_field_definition_per_kind`] (same
/// order) -- the fixture both the round-trip and overwrite contracts below
/// build on.
fn one_of_each_field_value(task_id: TaskId, defs: &[FieldDefinition; 7]) -> Vec<FieldValue> {
    let data = [
        FieldData::Number(NumberValue {
            units: 12345,
            scale: 2,
        }),
        FieldData::Currency(CurrencyAmount {
            minor_units: 1999,
            currency: CurrencyCode::new("USD").unwrap(),
        }),
        FieldData::Date(time::Date::from_calendar_date(2026, time::Month::March, 15).unwrap()),
        FieldData::Time(time::Time::from_hms(14, 30, 0).unwrap()),
        FieldData::DateTime(ts(1_700_000_000)),
        FieldData::Line("a one-liner".to_string()),
        FieldData::Block("a whole\nparagraph".to_string()),
    ];
    defs.iter()
        .zip(data)
        .map(|(def, data)| FieldValue {
            field_id: def.id,
            task_id,
            data,
        })
        .collect()
}

async fn field_value_contract(store: &SqlStore, project_id: ProjectId, task_id: TaskId) {
    let defs = seed_one_field_definition_per_kind(store, project_id).await;
    let values = one_of_each_field_value(task_id, &defs);
    for value in &values {
        TaskRepository::set_field_value(store, value).await.unwrap();
    }

    let loaded = TaskRepository::load(store, task_id).await.unwrap().unwrap();
    let mut loaded_values = loaded.field_values.clone();
    loaded_values.sort_by_key(|v| v.field_id.as_uuid());
    let mut expected_values = values.clone();
    expected_values.sort_by_key(|v| v.field_id.as_uuid());
    assert_eq!(
        loaded_values, expected_values,
        "every FieldKind variant must round-trip exactly through the typed EAV columns"
    );

    overwrite_field_value_contract(store, task_id, defs[0].id).await;
}

/// Overwriting an existing value updates the row rather than inserting a
/// duplicate -- distinct from the round-trip contract above, which only
/// covers first-write semantics.
async fn overwrite_field_value_contract(
    store: &SqlStore,
    task_id: TaskId,
    number_field_id: anamnesis_core::FieldId,
) {
    let updated_number = FieldValue {
        field_id: number_field_id,
        task_id,
        data: FieldData::Number(NumberValue { units: 1, scale: 0 }),
    };
    TaskRepository::set_field_value(store, &updated_number)
        .await
        .unwrap();
    let loaded = TaskRepository::load(store, task_id).await.unwrap().unwrap();
    assert_eq!(
        loaded.field_values.len(),
        7,
        "must overwrite, not duplicate"
    );
    assert!(loaded.field_values.contains(&updated_number));
}

// --- TaskRepository::update: optimistic concurrency ---

async fn optimistic_concurrency_contract(store: &SqlStore, seed: &Task) {
    let loaded = TaskRepository::load(store, seed.id).await.unwrap().unwrap();
    let original_touch = loaded.task.last_touched_at;

    let first_edit = anamnesis_core::edit_task(&loaded.task, "First edit", "", ts(5_000)).unwrap();
    TaskRepository::update(store, &first_edit, original_touch)
        .await
        .expect("the first update, with the correct expected timestamp, must succeed");

    // A second, concurrent editor who read the *original* (now-stale) task
    // tries to write using the timestamp they read it with. This is the
    // real conflict: two editors, one winner.
    let second_edit =
        anamnesis_core::edit_task(&loaded.task, "Second edit", "", ts(5_001)).unwrap();
    let result = TaskRepository::update(store, &second_edit, original_touch).await;
    assert!(
        matches!(result, Err(TaskUpdateError::Conflict)),
        "a write using a stale expected_last_touched_at must be rejected as a Conflict, got {result:?}"
    );

    // Nothing was written by the losing update.
    let after = TaskRepository::load(store, seed.id).await.unwrap().unwrap();
    assert_eq!(after.task.title.as_str(), "First edit");

    // The winner can now update again using the fresh timestamp.
    let third_edit = anamnesis_core::edit_task(&after.task, "Third edit", "", ts(5_002)).unwrap();
    TaskRepository::update(store, &third_edit, after.task.last_touched_at)
        .await
        .expect("updating with the current timestamp must succeed");

    // Updating a task that was never inserted is a repo error, not a
    // conflict (there is nothing to conflict with).
    let ghost = task(seed.project_id);
    let result = TaskRepository::update(store, &ghost, ts(0)).await;
    assert!(
        matches!(result, Err(TaskUpdateError::Repo(_))),
        "updating a nonexistent task must be a Repo error, got {result:?}"
    );
}

// --- Relationship ---

/// Inserts the two relationships every assertion below reads back: a
/// `BUILTIN_BLOCKS` edge from `b` to `a`, and a `BUILTIN_RELATES_TO` edge
/// from `a` to `b`.
async fn seed_blocks_and_relates(
    store: &SqlStore,
    a: &Task,
    b: &Task,
) -> (Relationship, Relationship) {
    let blocks = Relationship {
        id: Uuid::new_v4().into(),
        from_task_id: b.id,
        to_task_id: a.id,
        kind_id: KindId::BUILTIN_BLOCKS,
        created_at: ts(6_000),
    };
    let relates = Relationship {
        id: Uuid::new_v4().into(),
        from_task_id: a.id,
        to_task_id: b.id,
        kind_id: KindId::BUILTIN_RELATES_TO,
        created_at: ts(6_001),
    };
    RelationshipRepository::insert(store, &blocks)
        .await
        .unwrap();
    RelationshipRepository::insert(store, &relates)
        .await
        .unwrap();
    (blocks, relates)
}

async fn relationship_contract(store: &SqlStore, a: &Task, b: &Task) {
    let missing = RelationshipRepository::load(store, Uuid::new_v4().into())
        .await
        .unwrap();
    assert_eq!(missing, None);

    let (blocks, relates) = seed_blocks_and_relates(store, a, b).await;

    let loaded = RelationshipRepository::load(store, blocks.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded, blocks);

    let mut for_a = RelationshipRepository::list_for_task(store, a.id)
        .await
        .unwrap();
    for_a.sort_by_key(|r| r.id.as_uuid());
    let mut expected = vec![blocks, relates];
    expected.sort_by_key(|r| r.id.as_uuid());
    assert_eq!(
        for_a, expected,
        "a task appearing on either side must be found"
    );

    let blocking = RelationshipRepository::list_blocking(store).await.unwrap();
    assert_eq!(
        blocking.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![blocks.id],
        "list_blocking returns only the builtin blocks kind"
    );

    RelationshipRepository::delete(store, relates.id)
        .await
        .unwrap();
    assert_eq!(
        RelationshipRepository::load(store, relates.id)
            .await
            .unwrap(),
        None
    );
    // Deleting an already-absent relationship is not an error.
    RelationshipRepository::delete(store, relates.id)
        .await
        .unwrap();
}

// --- Tangle ---

/// Seeds an area, a project, three tasks and one active tangle over them --
/// the shared arrange step for every phase of the tangle contract below.
async fn seed_tangle(store: &SqlStore) -> (Tangle, BTreeSet<TaskId>) {
    let area_row = area(3);
    AreaRepository::insert(store, &area_row).await.unwrap();
    let p = project(area_row.id, ProjectStatus::Active);
    ProjectRepository::insert(store, &p).await.unwrap();
    let x = task(p.id);
    let y = task(p.id);
    let z = task(p.id);
    TaskRepository::insert(store, &x).await.unwrap();
    TaskRepository::insert(store, &y).await.unwrap();
    TaskRepository::insert(store, &z).await.unwrap();

    let task_ids: BTreeSet<TaskId> = [x.id, y.id, z.id].into_iter().collect();
    let tangle = Tangle {
        id: TangleId::new(Uuid::new_v4()),
        fingerprint: anamnesis_core::Fingerprint::of(&task_ids),
        task_ids: task_ids.clone(),
        placement: Placement::Below,
        frozen: false,
        detected_at: ts(7_000),
        resolved_at: None,
        archived_at: None,
    };
    TangleRepository::insert(store, &tangle).await.unwrap();
    (tangle, task_ids)
}

async fn tangle_contract(store: &SqlStore) {
    let (tangle, task_ids) = seed_tangle(store).await;

    let active = TangleRepository::list_active(store).await.unwrap();
    let found = active.iter().find(|t| t.id == tangle.id).unwrap();
    assert_eq!(found.task_ids, task_ids);
    assert_eq!(
        found.fingerprint,
        anamnesis_core::Fingerprint::of(&task_ids),
        "fingerprint is recomputed from the stored task_ids, not persisted directly"
    );
    assert_eq!(found.placement, Placement::Below);
    assert!(!found.frozen);

    let loaded = TangleRepository::load(store, tangle.id)
        .await
        .unwrap()
        .expect("load must find the tangle just inserted");
    assert_eq!(loaded.task_ids, task_ids);
    assert_eq!(loaded.placement, Placement::Below);

    assert!(
        TangleRepository::load(store, TangleId::new(Uuid::new_v4()))
            .await
            .unwrap()
            .is_none(),
        "load must return None for a tangle id that does not exist"
    );

    tangle_placement_contract(store, &tangle).await;
    tangle_archive_contract(store, &tangle).await;
}

/// Placing freezes it: `placement`/`frozen` round-trip through storage
/// exactly as `Task`'s do (`docs/DOMAIN.md`'s Tangle section). Dropping it
/// unfreezes it and moves it back below the horizon.
async fn tangle_placement_contract(store: &SqlStore, tangle: &Tangle) {
    let holding_column = column(9, None, false);
    store.seed_board_column(&holding_column).await.unwrap();
    let column_id = holding_column.id;
    let placed = anamnesis_core::place_tangle(tangle, column_id, 4).unwrap();
    TangleRepository::update(store, &placed).await.unwrap();
    let reloaded = TangleRepository::load(store, tangle.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        reloaded.placement,
        Placement::OnBoard {
            column: column_id,
            position: 4
        }
    );
    assert!(reloaded.frozen);

    let dropped = anamnesis_core::drop_tangle(&reloaded).unwrap();
    TangleRepository::update(store, &dropped).await.unwrap();
    let reloaded = TangleRepository::load(store, tangle.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.placement, Placement::Below);
    assert!(!reloaded.frozen);
}

/// Resolving hides a tangle from `list_active`; archiving additionally
/// round-trips `archived_at` and makes it vanish from its column's item
/// list (Gap 2).
async fn tangle_archive_contract(store: &SqlStore, tangle: &Tangle) {
    let mut resolved = tangle.clone();
    resolved.resolved_at = Some(ts(7_500));
    TangleRepository::update(store, &resolved).await.unwrap();
    let active = TangleRepository::list_active(store).await.unwrap();
    assert!(
        !active.iter().any(|t| t.id == tangle.id),
        "a resolved tangle must no longer be active"
    );

    assert_eq!(
        resolved.archived_at, None,
        "resolving alone must not archive it"
    );
    let archived = anamnesis_core::archive_tangle(&resolved, ts(7_600)).unwrap();
    TangleRepository::update(store, &archived).await.unwrap();

    let reloaded = TangleRepository::load(store, tangle.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        reloaded.archived_at,
        Some(ts(7_600)),
        "archived_at must round-trip through storage"
    );

    // Place it back on a column directly (bypassing `place_tangle`, which
    // rejects a resolved tangle) so the "vanishes from its column" half of
    // the assertion has a column to check against.
    let vanish_column = column(11, None, true);
    store.seed_board_column(&vanish_column).await.unwrap();
    let mut on_the_board = reloaded;
    on_the_board.placement = Placement::OnBoard {
        column: vanish_column.id,
        position: 0,
    };
    TangleRepository::update(store, &on_the_board)
        .await
        .unwrap();

    let columns = BoardQuery::columns_with_items(store).await.unwrap();
    let vanish = columns
        .iter()
        .find(|c| c.column.id == vanish_column.id)
        .unwrap();
    assert!(
        vanish.items.is_empty(),
        "an archived tangle must not render in its column: {:?}",
        vanish.items
    );
}

// --- BoardQuery + suggestion_candidates + blocking_graph ---

/// Three board columns, with two tasks on the first, inserted out of
/// position order to prove ordering is read back from `board_position`, not
/// insertion order.
async fn seed_todo_column_and_tasks(
    store: &SqlStore,
    project_id: ProjectId,
) -> (Column, Column, Column, Task, Task) {
    let todo = column(0, Some(1), false);
    let doing = column(1, None, false);
    let done = column(2, None, true);
    store.seed_board_column(&todo).await.unwrap();
    store.seed_board_column(&doing).await.unwrap();
    store.seed_board_column(&done).await.unwrap();

    let mut on_todo_first = task(project_id);
    on_todo_first.placement = Placement::OnBoard {
        column: todo.id,
        position: 1,
    };
    let mut on_todo_second = task(project_id);
    on_todo_second.placement = Placement::OnBoard {
        column: todo.id,
        position: 0,
    };
    TaskRepository::insert(store, &on_todo_first).await.unwrap();
    TaskRepository::insert(store, &on_todo_second)
        .await
        .unwrap();

    (todo, doing, done, on_todo_first, on_todo_second)
}

/// A done-column task blocking a below-the-horizon one, for the
/// `blocking_graph` and `suggestion_candidates` assertions.
async fn seed_blocking_pair(
    store: &SqlStore,
    project_id: ProjectId,
    done_column: anamnesis_core::ColumnId,
) -> (Task, Task) {
    let mut blocker = task(project_id);
    blocker.placement = Placement::OnBoard {
        column: done_column,
        position: 0,
    };
    TaskRepository::insert(store, &blocker).await.unwrap();

    let mut blocked = task(project_id);
    blocked.placement = Placement::Below;
    TaskRepository::insert(store, &blocked).await.unwrap();

    let edge = Relationship {
        id: Uuid::new_v4().into(),
        from_task_id: blocker.id,
        to_task_id: blocked.id,
        kind_id: KindId::BUILTIN_BLOCKS,
        created_at: ts(8_000),
    };
    RelationshipRepository::insert(store, &edge).await.unwrap();

    (blocker, blocked)
}

#[allow(clippy::type_complexity)]
async fn seed_board_and_blocking(
    store: &SqlStore,
    project_id: ProjectId,
) -> (Column, Column, Column, Task, Task, Task, Task) {
    let (todo, doing, done, on_todo_first, on_todo_second) =
        seed_todo_column_and_tasks(store, project_id).await;
    let (blocker, blocked) = seed_blocking_pair(store, project_id, done.id).await;
    (
        todo,
        doing,
        done,
        on_todo_first,
        on_todo_second,
        blocker,
        blocked,
    )
}

/// `columns_with_items` must order tasks within a column by board position,
/// and order the columns themselves by their own `position`.
async fn assert_board_ordering(
    store: &SqlStore,
    todo: &Column,
    doing: &Column,
    done: &Column,
    first: &Task,
    second: &Task,
) {
    let columns = BoardQuery::columns_with_items(store).await.unwrap();
    let todo_column = columns.iter().find(|c| c.column.id == todo.id).unwrap();
    let todo_task_ids: Vec<TaskId> = todo_column
        .items
        .iter()
        .map(|item| match item {
            anamnesis_app::BoardItem::Task(t) => t.id,
            anamnesis_app::BoardItem::Tangle(t) => panic!("expected only tasks here, got {t:?}"),
        })
        .collect();
    assert_eq!(
        todo_task_ids,
        vec![second.id, first.id],
        "tasks on a column must be ordered by board position"
    );
    let column_positions: Vec<u32> = columns
        .iter()
        .filter(|c| [todo.id, doing.id, done.id].contains(&c.column.id))
        .map(|c| c.column.position)
        .collect();
    assert_eq!(
        column_positions,
        vec![0, 1, 2],
        "columns must be ordered by position"
    );
}

async fn board_and_suggestion_contract(store: &SqlStore, project_id: ProjectId) {
    let (todo, doing, done, on_todo_first, on_todo_second, blocker, blocked) =
        seed_board_and_blocking(store, project_id).await;

    let count = BoardQuery::count_on_column(store, todo.id).await.unwrap();
    assert_eq!(count, 2);

    let state = BoardQuery::board_state(store, todo.id).await.unwrap();
    assert_eq!(state.wip_limit, Some(1));
    assert_eq!(state.current_count, 2);

    assert_board_ordering(store, &todo, &doing, &done, &on_todo_first, &on_todo_second).await;

    let graph: BlockingGraph = BoardQuery::blocking_graph(store).await.unwrap();
    assert!(graph.edges.contains(&(blocker.id, blocked.id)));
    assert!(
        graph.done_task_ids.contains(&blocker.id),
        "a task sitting in an is_done column is a done task"
    );
    assert!(!graph.done_task_ids.contains(&blocked.id));

    let candidates = BoardQuery::suggestion_candidates(store).await.unwrap();
    let candidate = candidates
        .iter()
        .find(|c| c.task_id == blocked.id)
        .expect("suggestion_candidates must include every non-archived task");
    assert_eq!(candidate.project_status, ProjectStatus::Active);
    assert_eq!(candidate.placement, Placement::Below);
}

// --- A placed tangle interleaves with tasks and counts against WIP
// (`docs/DOMAIN.md`'s Tangle section) ---

async fn tangle_on_board_contract(store: &SqlStore, project_id: ProjectId) {
    // A fresh, dedicated WIP-2 column so this test's counts cannot be
    // confused with `board_and_suggestion_contract`'s own column.
    let lane = column(50, Some(2), false);
    store.seed_board_column(&lane).await.unwrap();

    let mut solo_task = task(project_id);
    solo_task.placement = Placement::OnBoard {
        column: lane.id,
        position: 1,
    };
    TaskRepository::insert(store, &solo_task).await.unwrap();

    let x = task(project_id);
    let y = task(project_id);
    TaskRepository::insert(store, &x).await.unwrap();
    TaskRepository::insert(store, &y).await.unwrap();
    let knot: BTreeSet<TaskId> = [x.id, y.id].into_iter().collect();
    let tangle = anamnesis_core::place_tangle(
        &Tangle {
            id: TangleId::new(Uuid::new_v4()),
            fingerprint: anamnesis_core::Fingerprint::of(&knot),
            task_ids: knot,
            placement: Placement::Below,
            frozen: false,
            detected_at: ts(9_000),
            resolved_at: None,
            archived_at: None,
        },
        lane.id,
        0, // placed before the task, at position 0
    )
    .unwrap();
    TangleRepository::insert(store, &tangle).await.unwrap();

    // A placed tangle occupies a column slot and counts against the
    // column's WIP limit exactly like a task: one task + one tangle fills a
    // limit of 2.
    let count = BoardQuery::count_on_column(store, lane.id).await.unwrap();
    assert_eq!(
        count, 2,
        "a placed tangle must count against the column's WIP limit like a task"
    );
    let state = BoardQuery::board_state(store, lane.id).await.unwrap();
    assert_eq!(state.wip_limit, Some(2));
    assert_eq!(state.current_count, 2);

    // Tasks and tangles interleave correctly by position, not grouped by
    // kind: the tangle (position 0) comes before the task (position 1).
    let columns = BoardQuery::columns_with_items(store).await.unwrap();
    let lane_column = columns.iter().find(|c| c.column.id == lane.id).unwrap();
    assert_eq!(lane_column.items.len(), 2);
    match &lane_column.items[0] {
        anamnesis_app::BoardItem::Tangle(t) => assert_eq!(t.id, tangle.id),
        other => panic!("expected the tangle at position 0, got {other:?}"),
    }
    match &lane_column.items[1] {
        anamnesis_app::BoardItem::Task(t) => assert_eq!(t.id, solo_task.id),
        other => panic!("expected the task at position 1, got {other:?}"),
    }
}

// --- Comment ---

async fn comment_contract(store: &SqlStore, owner: &Task) {
    let first = Comment {
        id: CommentId::new(Uuid::new_v4()),
        task_id: owner.id,
        author: UserId::new("alice"),
        body: "first".to_string(),
        created_at: ts(9_000),
        edited_at: None,
    };
    let second = Comment {
        id: CommentId::new(Uuid::new_v4()),
        task_id: owner.id,
        author: UserId::new("bob"),
        body: "second".to_string(),
        created_at: ts(9_001),
        edited_at: None,
    };
    CommentRepository::insert(store, &first).await.unwrap();
    CommentRepository::insert(store, &second).await.unwrap();

    let loaded = CommentRepository::load(store, first.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded, first);

    let listed = CommentRepository::list_for_task(store, owner.id)
        .await
        .unwrap();
    assert_eq!(
        listed.iter().map(|c| c.id).collect::<Vec<_>>(),
        vec![first.id, second.id],
        "comments list ordered by created_at"
    );

    let mut edited = first.clone();
    edited.body = "first, edited".to_string();
    edited.edited_at = Some(ts(9_500));
    CommentRepository::update(store, &edited).await.unwrap();
    assert_eq!(
        CommentRepository::load(store, first.id).await.unwrap(),
        Some(edited)
    );

    CommentRepository::delete(store, second.id).await.unwrap();
    assert_eq!(
        CommentRepository::load(store, second.id).await.unwrap(),
        None
    );
}

// --- Attachment ---

async fn attachment_contract(store: &SqlStore, owner: &Task) {
    let link = Attachment {
        id: AttachmentId::new(Uuid::new_v4()),
        task_id: owner.id,
        kind: AttachmentKind::Link {
            url: "https://example.com/receipt".to_string(),
        },
        created_at: ts(10_000),
    };
    let file = Attachment {
        id: AttachmentId::new(Uuid::new_v4()),
        task_id: owner.id,
        kind: AttachmentKind::File {
            blob_key: "blobs/abc123".to_string(),
            filename: "photo.png".to_string(),
            mime: "image/png".to_string(),
            size: 2048,
        },
        created_at: ts(10_001),
    };
    AttachmentRepository::insert(store, &link).await.unwrap();
    AttachmentRepository::insert(store, &file).await.unwrap();

    assert_eq!(
        AttachmentRepository::load(store, link.id).await.unwrap(),
        Some(link.clone())
    );
    assert_eq!(
        AttachmentRepository::load(store, file.id).await.unwrap(),
        Some(file.clone())
    );

    let listed = AttachmentRepository::list_for_task(store, owner.id)
        .await
        .unwrap();
    assert_eq!(
        listed.iter().map(|a| a.id).collect::<Vec<_>>(),
        vec![link.id, file.id]
    );

    AttachmentRepository::delete(store, link.id).await.unwrap();
    assert_eq!(
        AttachmentRepository::load(store, link.id).await.unwrap(),
        None
    );
}

// --- Membership: system admin, area/project roles, inheritance + override ---

async fn membership_contract(store: &SqlStore) {
    let admin = UserId::new(format!("admin-{}", Uuid::new_v4()));
    let member = UserId::new(format!("member-{}", Uuid::new_v4()));
    let stranger = UserId::new(format!("stranger-{}", Uuid::new_v4()));

    let a = area(20);
    AreaRepository::insert(store, &a).await.unwrap();
    let p = project(a.id, ProjectStatus::Pending);
    ProjectRepository::insert(store, &p).await.unwrap();

    system_admin_grant_contract(store, &admin, &member).await;
    membership_inheritance_contract(store, &member, &stranger, &admin, a.id, p.id).await;
    membership_listing_and_revocation_contract(store, &admin, &member, &stranger, a.id, p.id).await;
}

async fn system_admin_grant_contract(store: &SqlStore, admin: &UserId, member: &UserId) {
    store.grant_system_admin(admin).await.unwrap();
    assert!(
        MembershipQuery::is_system_admin(store, admin)
            .await
            .unwrap()
    );
    assert!(
        !MembershipQuery::is_system_admin(store, member)
            .await
            .unwrap()
    );
}

/// Area/project role inheritance: no explicit project role falls back to
/// the area role; strongest wins, not most specific, in either direction;
/// and a stranger/System Admin both resolve to their expected default with
/// no membership row of their own.
async fn membership_inheritance_contract(
    store: &SqlStore,
    member: &UserId,
    stranger: &UserId,
    admin: &UserId,
    area_id: anamnesis_core::AreaId,
    project_id: ProjectId,
) {
    membership_area_role_fallback_contract(store, member, area_id, project_id).await;
    membership_strongest_wins_contract(store, member, area_id, project_id).await;
    membership_default_roles_contract(store, stranger, admin, area_id, project_id).await;
}

/// No explicit project role -> the area role applies.
async fn membership_area_role_fallback_contract(
    store: &SqlStore,
    member: &UserId,
    area_id: anamnesis_core::AreaId,
    project_id: ProjectId,
) {
    store
        .set_area_role(member, area_id, Role::Member)
        .await
        .unwrap();
    assert!(matches!(
        MembershipQuery::area_role(store, member, area_id)
            .await
            .unwrap(),
        Some(Role::Member)
    ));
    assert_eq!(
        MembershipQuery::project_role(store, member, project_id)
            .await
            .unwrap(),
        None,
        "an area role is not itself a project role"
    );

    let effective = MembershipQuery::effective_role(store, member, project_id, area_id)
        .await
        .unwrap();
    assert!(matches!(effective, Some(Role::Member)));
}

/// Strongest wins, not most specific, in either direction -- grants are
/// independent and stack, by analogy to `chmod` (adding a grant must never
/// subtract capability).
async fn membership_strongest_wins_contract(
    store: &SqlStore,
    member: &UserId,
    area_id: anamnesis_core::AreaId,
    project_id: ProjectId,
) {
    store
        .set_area_role(member, area_id, Role::ProjectAdmin)
        .await
        .unwrap();
    store
        .set_project_role(member, project_id, Role::Member)
        .await
        .unwrap();
    let effective = MembershipQuery::effective_role(store, member, project_id, area_id)
        .await
        .unwrap();
    assert!(
        matches!(effective, Some(Role::ProjectAdmin)),
        "a lower explicit project role must not demote a higher area role"
    );

    store
        .set_area_role(member, area_id, Role::Member)
        .await
        .unwrap();
    store
        .set_project_role(member, project_id, Role::ProjectAdmin)
        .await
        .unwrap();
    let effective = MembershipQuery::effective_role(store, member, project_id, area_id)
        .await
        .unwrap();
    assert!(
        matches!(effective, Some(Role::ProjectAdmin)),
        "a higher explicit project role must still elevate above a lower area role"
    );
}

/// A stranger with no rows anywhere and no system admin grant has no
/// effective role at all; System Admin resolves through the same query even
/// with no membership row anywhere.
async fn membership_default_roles_contract(
    store: &SqlStore,
    stranger: &UserId,
    admin: &UserId,
    area_id: anamnesis_core::AreaId,
    project_id: ProjectId,
) {
    assert_eq!(
        MembershipQuery::effective_role(store, stranger, project_id, area_id)
            .await
            .unwrap(),
        None
    );

    let admin_effective = MembershipQuery::effective_role(store, admin, project_id, area_id)
        .await
        .unwrap();
    assert!(matches!(admin_effective, Some(Role::SystemAdmin)));
}

/// Gap 1: `MembershipRepository`'s listing queries reflect the roles set up
/// by `membership_inheritance_contract`, and its revocations actually take
/// effect -- including as harmless no-ops for a grant nobody holds.
async fn membership_listing_and_revocation_contract(
    store: &SqlStore,
    admin: &UserId,
    member: &UserId,
    stranger: &UserId,
    area_id: anamnesis_core::AreaId,
    project_id: ProjectId,
) {
    membership_listing_contract(store, admin, member, area_id, project_id).await;
    membership_revocation_contract(store, admin, member, stranger, area_id, project_id).await;
}

async fn membership_listing_contract(
    store: &SqlStore,
    admin: &UserId,
    member: &UserId,
    area_id: anamnesis_core::AreaId,
    project_id: ProjectId,
) {
    let admins = MembershipQuery::list_system_admins(store).await.unwrap();
    assert!(admins.contains(admin));
    assert!(!admins.contains(member));

    let area_members = MembershipQuery::list_area_members(store, area_id)
        .await
        .unwrap();
    assert!(
        area_members.contains(&(member.clone(), Role::Member)),
        "expected {member:?} with Role::Member in {area_members:?}"
    );

    let project_members = MembershipQuery::list_project_members(store, project_id)
        .await
        .unwrap();
    assert!(
        project_members.contains(&(member.clone(), Role::ProjectAdmin)),
        "expected {member:?} with Role::ProjectAdmin in {project_members:?}"
    );
}

async fn membership_revocation_contract(
    store: &SqlStore,
    admin: &UserId,
    member: &UserId,
    stranger: &UserId,
    area_id: anamnesis_core::AreaId,
    project_id: ProjectId,
) {
    member_role_revocation_contract(store, member, area_id, project_id).await;
    admin_and_stranger_revocation_contract(store, admin, stranger, area_id, project_id).await;
}

/// Revoking a member's area and project roles actually clears both rows, and
/// with neither left, `effective_role` reports no role at all.
async fn member_role_revocation_contract(
    store: &SqlStore,
    member: &UserId,
    area_id: anamnesis_core::AreaId,
    project_id: ProjectId,
) {
    MembershipRepository::revoke_area_role(store, member, area_id)
        .await
        .unwrap();
    assert_eq!(
        MembershipQuery::area_role(store, member, area_id)
            .await
            .unwrap(),
        None,
        "revoke_area_role must actually clear the row"
    );

    MembershipRepository::revoke_project_role(store, member, project_id)
        .await
        .unwrap();
    assert_eq!(
        MembershipQuery::project_role(store, member, project_id)
            .await
            .unwrap(),
        None,
        "revoke_project_role must actually clear the row"
    );

    assert_eq!(
        MembershipQuery::effective_role(store, member, project_id, area_id)
            .await
            .unwrap(),
        None
    );
}

/// Revoking an admin's system-admin grant clears it from both the direct
/// check and the listing; revoking any grant a stranger never held is a
/// harmless no-op, not an error.
async fn admin_and_stranger_revocation_contract(
    store: &SqlStore,
    admin: &UserId,
    stranger: &UserId,
    area_id: anamnesis_core::AreaId,
    project_id: ProjectId,
) {
    MembershipRepository::revoke_system_admin(store, admin)
        .await
        .unwrap();
    assert!(
        !MembershipQuery::is_system_admin(store, admin)
            .await
            .unwrap(),
        "revoke_system_admin must actually clear the grant"
    );
    let admins_after = MembershipQuery::list_system_admins(store).await.unwrap();
    assert!(!admins_after.contains(admin));

    // Revoking a grant nobody holds is a harmless no-op, not an error.
    MembershipRepository::revoke_system_admin(store, stranger)
        .await
        .unwrap();
    MembershipRepository::revoke_area_role(store, stranger, area_id)
        .await
        .unwrap();
    MembershipRepository::revoke_project_role(store, stranger, project_id)
        .await
        .unwrap();
}

// --- Search: SearchIndex (write) + SearchQuery (read), across kinds ---

async fn search_contract(store: &SqlStore) {
    let area_id: anamnesis_core::AreaId = Uuid::new_v4().into();
    let project_id: ProjectId = Uuid::new_v4().into();
    let task_id: TaskId = Uuid::new_v4().into();

    search_index_all_kinds_contract(store, area_id, project_id, task_id).await;
    search_reindex_and_archive_contract(store, area_id, project_id, task_id).await;
    search_prefix_and_edge_case_contract(store).await;
}

/// Indexing an area, a project and a task each makes them findable by a
/// shared distinctive whole word -- see `crate::sql::search`'s module doc
/// comment on why it must be a whole word, not a substring: FTS5/tsvector
/// both match tokens.
async fn search_index_all_kinds_contract(
    store: &SqlStore,
    area_id: anamnesis_core::AreaId,
    project_id: ProjectId,
    task_id: TaskId,
) {
    SearchIndex::index_area(store, area_id, "Zylophone practice area")
        .await
        .unwrap();
    SearchIndex::index_project(store, project_id, "Zylophone repair project")
        .await
        .unwrap();
    SearchIndex::index_task(store, task_id, "Buy zylophone mallets")
        .await
        .unwrap();

    let hits = SearchQuery::search(store, "zylophone").await.unwrap();
    assert_eq!(hits.len(), 3, "all three kinds must be found: {hits:?}");
    assert!(hits.contains(&SearchHit::Area {
        id: area_id,
        title: "Zylophone practice area".to_string()
    }));
    assert!(hits.contains(&SearchHit::Project {
        id: project_id,
        title: "Zylophone repair project".to_string()
    }));
    assert!(hits.contains(&SearchHit::Task {
        id: task_id,
        title: "Buy zylophone mallets".to_string()
    }));
}

/// Re-indexing updates a title rather than duplicating it; `remove_*`
/// archives rather than deletes (`SearchIndex`'s trait doc comment:
/// `docs/DOMAIN.md` §2's "vanished... unless explicitly searched" requires
/// an explicit path back to them) -- `search_archived` is that path; and
/// re-indexing an archived entity (the unarchive path) moves it back from
/// `search_archived` to plain `search`.
async fn search_reindex_and_archive_contract(
    store: &SqlStore,
    area_id: anamnesis_core::AreaId,
    project_id: ProjectId,
    task_id: TaskId,
) {
    search_reindex_and_removal_contract(store, area_id, project_id, task_id).await;
    search_archived_round_trip_contract(store, area_id, project_id, task_id).await;
}

/// Re-indexing drops the old title from search; removing (archiving) an
/// entity hides it from plain search entirely.
async fn search_reindex_and_removal_contract(
    store: &SqlStore,
    area_id: anamnesis_core::AreaId,
    project_id: ProjectId,
    task_id: TaskId,
) {
    SearchIndex::index_task(store, task_id, "Buy zylophone stands")
        .await
        .unwrap();
    let hits = SearchQuery::search(store, "mallets").await.unwrap();
    assert!(
        hits.is_empty(),
        "the old title must no longer be findable after re-indexing"
    );
    let hits = SearchQuery::search(store, "stands").await.unwrap();
    assert_eq!(hits.len(), 1);

    SearchIndex::remove_task(store, task_id).await.unwrap();
    SearchIndex::remove_project(store, project_id)
        .await
        .unwrap();
    SearchIndex::remove_area(store, area_id).await.unwrap();
    let hits = SearchQuery::search(store, "zylophone").await.unwrap();
    assert!(
        hits.is_empty(),
        "archived entities must not appear in plain search"
    );
}

/// `search_archived` finds all three entities removed just above, and
/// re-indexing the task both restores it to plain search and drops it back
/// out of `search_archived`.
async fn search_archived_round_trip_contract(
    store: &SqlStore,
    area_id: anamnesis_core::AreaId,
    project_id: ProjectId,
    task_id: TaskId,
) {
    let archived_hits = SearchQuery::search_archived(store, "zylophone")
        .await
        .unwrap();
    assert_eq!(
        archived_hits.len(),
        3,
        "search_archived must find all three archived entities: {archived_hits:?}"
    );
    assert!(archived_hits.contains(&SearchHit::Area {
        id: area_id,
        title: "Zylophone practice area".to_string()
    }));
    assert!(archived_hits.contains(&SearchHit::Project {
        id: project_id,
        title: "Zylophone repair project".to_string()
    }));
    assert!(archived_hits.contains(&SearchHit::Task {
        id: task_id,
        title: "Buy zylophone stands".to_string()
    }));

    SearchIndex::index_task(store, task_id, "Buy zylophone stands")
        .await
        .unwrap();
    let hits = SearchQuery::search(store, "stands").await.unwrap();
    assert_eq!(hits.len(), 1, "unarchiving must restore it to plain search");
    let archived_hits = SearchQuery::search_archived(store, "stands").await.unwrap();
    assert!(
        archived_hits.is_empty(),
        "an unarchived entity must no longer appear in search_archived"
    );
}

/// A blank query returns no hits rather than every row, for both paths.
/// Live search-as-you-type queries on every keystroke, so a partial word
/// must prefix-match a task title as the user is still typing it -- but a
/// mid-word substring, or punctuation-only input, must degrade to no
/// results rather than erroring the query.
async fn search_prefix_and_edge_case_contract(store: &SqlStore) {
    assert_eq!(SearchQuery::search(store, "").await.unwrap(), Vec::new());
    assert_eq!(
        SearchQuery::search_archived(store, "").await.unwrap(),
        Vec::new()
    );

    let prefix_task_id: TaskId = Uuid::new_v4().into();
    SearchIndex::index_task(store, prefix_task_id, "test1")
        .await
        .unwrap();
    let hits = SearchQuery::search(store, "tes").await.unwrap();
    assert_eq!(
        hits,
        vec![SearchHit::Task {
            id: prefix_task_id,
            title: "test1".to_string()
        }],
        "a partial word must prefix-match a task title as the user is still typing it"
    );

    let no_hits = SearchQuery::search(store, "est1").await.unwrap();
    assert!(
        no_hits.is_empty(),
        "a mid-word substring must not match, only a leading prefix: {no_hits:?}"
    );

    let hits = SearchQuery::search(store, "!!!").await.unwrap();
    assert!(hits.is_empty());

    SearchIndex::remove_task(store, prefix_task_id)
        .await
        .unwrap();
}

// --- Settings ---

fn default_settings() -> Settings {
    Settings {
        active_project_limit: 5,
        suggestion: SuggestionSettings {
            cooldown_seconds: 259_200,
            high_bounce_threshold: 3,
        },
        sweep_recurrence: Recurrence::Never,
        last_swept_at: None,
    }
}

/// Seeding is idempotent: a second seed call must not clobber a value
/// already changed by `update` in between (mirrors
/// `anamnesis-web::bootstrap`'s own idempotency requirement).
async fn seed_and_verify_default_settings(store: &SqlStore, defaults: Settings) {
    store
        .seed_settings_if_missing(&defaults, "UTC")
        .await
        .unwrap();
    let loaded = SettingsRepository::load(store).await.unwrap();
    assert_eq!(loaded, defaults);
}

/// `update` round-trips every editable field, including a real
/// `EveryNWeeks` recurrence (exercising the weekday encode/decode path, not
/// just `Never`/`DayOfMonth`) and then a `DayOfMonth` recurrence too.
async fn settings_update_round_trip_contract(store: &SqlStore) -> Settings {
    let edited = Settings {
        active_project_limit: 11,
        suggestion: SuggestionSettings {
            cooldown_seconds: 42,
            high_bounce_threshold: 9,
        },
        sweep_recurrence: Recurrence::EveryNWeeks {
            n: 2,
            weekday: time::Weekday::Monday,
        },
        last_swept_at: None, // `update` must never write this field.
    };
    SettingsRepository::update(store, &edited).await.unwrap();
    let reloaded = SettingsRepository::load(store).await.unwrap();
    assert_eq!(reloaded.active_project_limit, 11);
    assert_eq!(reloaded.suggestion.cooldown_seconds, 42);
    assert_eq!(reloaded.suggestion.high_bounce_threshold, 9);
    assert_eq!(
        reloaded.sweep_recurrence,
        Recurrence::EveryNWeeks {
            n: 2,
            weekday: time::Weekday::Monday
        }
    );
    assert_eq!(
        reloaded.last_swept_at, None,
        "update must not touch last_swept_at"
    );

    let day_of_month = Settings {
        sweep_recurrence: Recurrence::DayOfMonth { day: 15 },
        ..edited
    };
    SettingsRepository::update(store, &day_of_month)
        .await
        .unwrap();
    let reloaded = SettingsRepository::load(store).await.unwrap();
    assert_eq!(
        reloaded.sweep_recurrence,
        Recurrence::DayOfMonth { day: 15 }
    );

    day_of_month
}

/// `record_sweep` writes only `last_swept_at`, leaving every other field
/// exactly as `settings_update_round_trip_contract` last left it.
async fn settings_record_sweep_contract(store: &SqlStore, current: Settings) -> Settings {
    let swept_at = ts(123_456);
    SettingsRepository::record_sweep(store, swept_at)
        .await
        .unwrap();
    let after_sweep = SettingsRepository::load(store).await.unwrap();
    assert_eq!(after_sweep.last_swept_at, Some(swept_at));
    assert_eq!(
        after_sweep.active_project_limit,
        current.active_project_limit
    );
    assert_eq!(after_sweep.sweep_recurrence, current.sweep_recurrence);
    after_sweep
}

/// Seeding again now that a real row (and a real edit) exists must be a
/// pure no-op -- the whole point of "if missing".
async fn reseed_after_edit_is_noop_contract(
    store: &SqlStore,
    defaults: Settings,
    after_sweep: Settings,
) {
    store
        .seed_settings_if_missing(&defaults, "UTC")
        .await
        .unwrap();
    let still_after_sweep = SettingsRepository::load(store).await.unwrap();
    assert_eq!(
        still_after_sweep, after_sweep,
        "seeding a second time must not overwrite an already-edited settings row"
    );
}

async fn settings_contract(store: &SqlStore) {
    let defaults = default_settings();
    seed_and_verify_default_settings(store, defaults).await;
    let edited = settings_update_round_trip_contract(store).await;
    let after_sweep = settings_record_sweep_contract(store, edited).await;
    reseed_after_edit_is_noop_contract(store, defaults, after_sweep).await;
}

// --- Job leases ---

/// How long every lease taken below is claimed for. A constant rather than a
/// parameter because the interesting number in each assertion is *when* the
/// claim is made, not how long it lasts.
const LEASE_TTL: Duration = Duration::from_secs(60);

/// Both backends must agree on when a lease blocks a rival and when it does
/// not, since the whole point of the lease is that N instances — which may be
/// running against either backend — reach the same answer.
///
/// The two halves take separate jobs and are independent: expiry is about
/// `try_acquire` deciding against the clock, release is about `release`
/// handing the job over early. Neither inherits the other's leftover state.
async fn job_lease_contract(store: &SqlStore) {
    let leases = store.job_lease().await.expect("open the job-lease store");
    lease_expiry_contract(&leases).await;
    lease_release_contract(&leases).await;
}

/// A fresh job name per run: unlike the SQLite temp file, a Postgres scratch
/// database keeps its `job_leases` rows between runs, and the truncation the
/// contract does at the top deliberately covers only domain tables.
fn scratch_job() -> String {
    format!("contract-{}", Uuid::new_v4())
}

/// `try_acquire` reduced to the question each assertion actually asks: as of
/// `at`, can `owner` hold `job`?
///
/// Time is a parameter throughout — never `Clock::now` — so expiry is tested
/// by naming a later instant rather than by sleeping.
async fn claims(leases: &dyn JobLease, job: &str, owner: &str, at: i64) -> bool {
    leases
        .try_acquire(job, owner, ts(at), LEASE_TTL)
        .await
        .expect("try_acquire must not fail")
}

/// Who holds the lease is decided by the clock: a live claim blocks, a lapsed
/// one does not, and the holder can push its own expiry out by renewing.
async fn lease_expiry_contract(leases: &dyn JobLease) {
    let job = scratch_job();

    assert!(
        claims(leases, &job, "a", 10_000).await,
        "an unclaimed job must be claimable"
    );
    assert!(
        !claims(leases, &job, "b", 10_030).await,
        "a live lease must block a second owner"
    );
    assert!(
        claims(leases, &job, "a", 10_030).await,
        "the current holder must be able to renew"
    );
    assert!(
        !claims(leases, &job, "b", 10_061).await,
        "the renewal must have pushed expiry out to 10_090, so a rival that \
         would have won against the original 10_060 expiry must still lose"
    );
    assert!(
        claims(leases, &job, "b", 10_091).await,
        "an expired lease must not block"
    );

    leases.release(&job, "b").await.unwrap();
}

/// `release` hands the job over immediately rather than at expiry — and only
/// the actual holder's release counts, so a straggler cannot free a lease it
/// no longer owns.
async fn lease_release_contract(leases: &dyn JobLease) {
    let job = scratch_job();

    assert!(claims(leases, &job, "a", 20_000).await);
    leases.release(&job, "a").await.unwrap();
    assert!(
        claims(leases, &job, "b", 20_010).await,
        "a released lease must be claimable at once, well inside the TTL it \
         would otherwise have run for"
    );

    leases
        .release(&job, "a")
        .await
        .expect("releasing a lease someone else holds is a no-op, not an error");
    assert!(
        !claims(leases, &job, "a", 20_020).await,
        "a's release must not have removed b's lease"
    );

    leases.release(&job, "b").await.unwrap();
}

/// Two processes starting against one fresh SQLite file at the same instant
/// must both come up. This is the same-machine multi-process topology's very
/// first moment, and it is entirely a `SqlStore::connect` concern — the
/// journal-mode conversion and the unlocked SQLite migrator both happen there,
/// before any port is ever called.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_concurrent_connects_to_a_fresh_sqlite_file_both_succeed() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let url = format!(
        "sqlite://{}?mode=rwc",
        dir.path().join("connect-race.db").display()
    );

    let (a, b) = tokio::join!(SqlStore::connect(&url), SqlStore::connect(&url));
    a.expect("first instance connects and migrates");
    b.expect("second instance connects and migrates");
}

/// Enough concurrent starts to make the window reliable rather than
/// occasional. Two is the real-world minimum but races only sometimes.
const CONCURRENT_INSTANCES: usize = 4;

/// The sharper version of the test above: several instances starting together
/// against a database that already carries sqlx's bookkeeping table but has
/// migrations outstanding — a rolling upgrade, rather than a first boot.
///
/// That distinction is what makes this deterministic instead of occasional.
/// `ensure_migrations_table` is itself a write, so on a genuinely fresh file
/// the instances queue behind it and the winner usually commits its migration
/// before the others get as far as reading what has been applied — the race is
/// hidden by an accident of locking. Creating that table up front removes the
/// accidental barrier and leaves nothing between the instances and sqlx's
/// unprotected check-then-act, which is also the real deployment shape: a
/// database being upgraded has had that table since its first boot.
///
/// Without `SqlStore::connect`'s migration lease every run of this fails with
/// `(code: 1) table areas already exists` — `SQLITE_ERROR`, not `SQLITE_BUSY`,
/// which is why no `busy_timeout` and no lock-contention retry covers it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn several_instances_upgrading_one_sqlite_file_at_once_all_succeed() {
    use std::str::FromStr;

    use sqlx::migrate::Migrate;

    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("upgrade-race.db");
    let url = format!("sqlite://{}?mode=rwc", path.display());

    // Setup, deliberately not part of the race: WAL (under the default
    // rollback journal a writer blocks readers, which would serialise the
    // check-then-act by accident) and sqlx's bookkeeping table.
    let options = sqlx::sqlite::SqliteConnectOptions::from_str(&url)
        .expect("parse sqlite url")
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
    let setup = sqlx::SqlitePool::connect_with(options)
        .await
        .expect("create the database in WAL mode");
    {
        let mut conn = setup.acquire().await.expect("setup connection");
        (*conn)
            .ensure_migrations_table("_sqlx_migrations")
            .await
            .expect("bookkeeping table");
    }
    setup.close().await;

    // Separate tasks, not `tokio::join!`: joined futures share one task and
    // interleave only at await points, which is too gentle to expose this.
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(CONCURRENT_INSTANCES));
    let mut instances = Vec::with_capacity(CONCURRENT_INSTANCES);
    for _ in 0..CONCURRENT_INSTANCES {
        let url = url.clone();
        let barrier = std::sync::Arc::clone(&barrier);
        instances.push(tokio::spawn(async move {
            barrier.wait().await;
            SqlStore::connect(&url).await
        }));
    }
    for instance in instances {
        instance
            .await
            .expect("instance panicked")
            .expect("instance connects and migrates");
    }

    // And the schema it produced is usable, not merely present.
    let store = SqlStore::connect(&url).await.expect("verifying connect");
    store.columns_with_items().await.expect("query the board");
}

#[tokio::test]
async fn sqlite_store_contract() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("anamnesis-domain-test.db");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());

    let store = SqlStore::connect(&url)
        .await
        .expect("connect + migrate sqlite store");

    contract(&store).await;
}

#[tokio::test]
#[ignore = "requires a live Postgres server; set ANAMNESIS_TEST_PG_URL and pass --ignored"]
async fn postgres_store_contract() {
    let Ok(url) = std::env::var("ANAMNESIS_TEST_PG_URL") else {
        eprintln!("skipping postgres_store_contract: ANAMNESIS_TEST_PG_URL is not set");
        return;
    };

    let store = SqlStore::connect(&url)
        .await
        .expect("connect + migrate postgres store");

    // Unlike the SQLite test (a fresh temp file every run), a Postgres URL
    // usually names a persistent scratch database that survives between
    // test runs. Wipe every domain table first so the contract's global
    // assertions (`count_active(None)`, `blocking_graph`'s system-wide
    // scan, ...) see only what this run inserts, not leftovers from a
    // previous run against the same database.
    let raw = sqlx::PgPool::connect(&url)
        .await
        .expect("connect a raw pool to reset the schema");
    sqlx::query(
        "TRUNCATE TABLE tangle_tasks, tangles, field_values, relationships, comments, \
         attachments, search_documents, tasks, field_definitions, relationship_kinds, \
         area_members, project_members, system_admins, projects, areas, board_columns, \
         settings \
         RESTART IDENTITY CASCADE",
    )
    .execute(&raw)
    .await
    .expect("truncate domain tables before the contract run");
    raw.close().await;

    contract(&store).await;
}
