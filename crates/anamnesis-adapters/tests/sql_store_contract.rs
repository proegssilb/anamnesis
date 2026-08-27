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

use anamnesis_adapters::SqlStore;
use anamnesis_app::{
    AreaRepository, Attachment, AttachmentId, AttachmentKind, AttachmentRepository, BoardQuery,
    Comment, CommentId, CommentRepository, MembershipQuery, ProjectAggregate, ProjectRepository,
    RelationshipRepository, SearchHit, SearchIndex, SearchQuery, TangleRepository, TaskAggregate,
    TaskRepository, TaskUpdateError,
};
use anamnesis_core::policy::Role;
use anamnesis_core::{
    Area, BlockingGraph, Column, CurrencyAmount, CurrencyCode, FieldData, FieldDefinition,
    FieldKind, FieldValue, KindId, NumberValue, Placement, Project, ProjectId, ProjectStatus,
    Relationship, RelationshipKind, Tangle, TangleId, Task, TaskId, Timestamp, Title, UserId,
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

/// The shared contract. Called once per backend so the two implementations
/// cannot drift apart.
async fn contract(store: &SqlStore) {
    area_contract(store).await;
    project_contract(store).await;
    let (task_contract_project, task_a, task_b) = task_and_field_value_contract(store).await;
    optimistic_concurrency_contract(store, &task_a).await;
    relationship_contract(store, &task_a, &task_b).await;
    tangle_contract(store).await;
    board_and_suggestion_contract(store, task_contract_project).await;
    comment_contract(store, &task_a).await;
    attachment_contract(store, &task_a).await;
    membership_contract(store).await;
    search_contract(store).await;
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

async fn project_contract(store: &SqlStore) {
    let owning_area = area(0);
    AreaRepository::insert(store, &owning_area).await.unwrap();

    let p = project(owning_area.id, ProjectStatus::Pending);
    ProjectRepository::insert(store, &p).await.unwrap();

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

    let kind = RelationshipKind {
        id: Uuid::new_v4().into(),
        project_id: Some(p.id),
        forward_label: title("inspired by"),
        reverse_label: title("inspired"),
    };
    ProjectRepository::insert_relationship_kind(store, &kind)
        .await
        .unwrap();

    let loaded = ProjectRepository::load(store, p.id).await.unwrap().unwrap();
    assert_eq!(
        loaded.field_definitions,
        vec![first.clone(), second.clone()],
        "field definitions load ordered by position, not insertion order"
    );
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

    // count_active / active-project-limit support.
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

// --- Task (+ field values, typed EAV) + list_children ordering ---

async fn task_and_field_value_contract(store: &SqlStore) -> (ProjectId, Task, Task) {
    let owning_area = area(2);
    AreaRepository::insert(store, &owning_area).await.unwrap();
    let mut p = project(owning_area.id, ProjectStatus::Active);
    p.status = ProjectStatus::Active;
    ProjectRepository::insert(store, &p).await.unwrap();

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

    // One field definition per FieldKind, to round-trip every variant of
    // the typed EAV encoding.
    let number_def = field_def(p.id, FieldKind::Number, 0, "Weight");
    let currency_def = field_def(p.id, FieldKind::Currency, 1, "Cost");
    let date_def = field_def(p.id, FieldKind::Date, 2, "Due");
    let time_def = field_def(p.id, FieldKind::Time, 3, "Reminder");
    let datetime_def = field_def(p.id, FieldKind::DateTime, 4, "Scheduled");
    let line_def = field_def(p.id, FieldKind::Line, 5, "Summary");
    let block_def = field_def(p.id, FieldKind::Block, 6, "Notes");
    for def in [
        &number_def,
        &currency_def,
        &date_def,
        &time_def,
        &datetime_def,
        &line_def,
        &block_def,
    ] {
        ProjectRepository::insert_field_definition(store, def)
            .await
            .unwrap();
    }

    let values = vec![
        FieldValue {
            field_id: number_def.id,
            task_id: a.id,
            data: FieldData::Number(NumberValue {
                units: 12345,
                scale: 2,
            }),
        },
        FieldValue {
            field_id: currency_def.id,
            task_id: a.id,
            data: FieldData::Currency(CurrencyAmount {
                minor_units: 1999,
                currency: CurrencyCode::new("USD").unwrap(),
            }),
        },
        FieldValue {
            field_id: date_def.id,
            task_id: a.id,
            data: FieldData::Date(
                time::Date::from_calendar_date(2026, time::Month::March, 15).unwrap(),
            ),
        },
        FieldValue {
            field_id: time_def.id,
            task_id: a.id,
            data: FieldData::Time(time::Time::from_hms(14, 30, 0).unwrap()),
        },
        FieldValue {
            field_id: datetime_def.id,
            task_id: a.id,
            data: FieldData::DateTime(ts(1_700_000_000)),
        },
        FieldValue {
            field_id: line_def.id,
            task_id: a.id,
            data: FieldData::Line("a one-liner".to_string()),
        },
        FieldValue {
            field_id: block_def.id,
            task_id: a.id,
            data: FieldData::Block("a whole\nparagraph".to_string()),
        },
    ];
    for value in &values {
        TaskRepository::set_field_value(store, value).await.unwrap();
    }

    let loaded = TaskRepository::load(store, a.id).await.unwrap().unwrap();
    let mut loaded_values = loaded.field_values.clone();
    loaded_values.sort_by_key(|v| v.field_id.as_uuid());
    let mut expected_values = values.clone();
    expected_values.sort_by_key(|v| v.field_id.as_uuid());
    assert_eq!(
        loaded_values, expected_values,
        "every FieldKind variant must round-trip exactly through the typed EAV columns"
    );

    // Overwriting an existing value (not inserting a duplicate row).
    let updated_number = FieldValue {
        field_id: number_def.id,
        task_id: a.id,
        data: FieldData::Number(NumberValue { units: 1, scale: 0 }),
    };
    TaskRepository::set_field_value(store, &updated_number)
        .await
        .unwrap();
    let loaded = TaskRepository::load(store, a.id).await.unwrap().unwrap();
    assert_eq!(
        loaded.field_values.len(),
        7,
        "must overwrite, not duplicate"
    );
    assert!(loaded.field_values.contains(&updated_number));

    // Checklist containment + ordering: two children, inserted out of
    // order, must list back ordered by checklist_position.
    let mut child_a = task(p.id);
    child_a.parent_task_id = Some(a.id);
    child_a.checklist_position = 1;
    let mut child_b = task(p.id);
    child_b.parent_task_id = Some(a.id);
    child_b.checklist_position = 0;
    TaskRepository::insert(store, &child_a).await.unwrap();
    TaskRepository::insert(store, &child_b).await.unwrap();

    let children = TaskRepository::list_children(store, a.id).await.unwrap();
    assert_eq!(
        children.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![child_b.id, child_a.id],
        "children must list ordered by checklist_position, not insertion order"
    );

    (p.id, a, b)
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

async fn relationship_contract(store: &SqlStore, a: &Task, b: &Task) {
    let missing = RelationshipRepository::load(store, Uuid::new_v4().into())
        .await
        .unwrap();
    assert_eq!(missing, None);

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

async fn tangle_contract(store: &SqlStore) {
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
        detected_at: ts(7_000),
        resolved_at: None,
    };
    TangleRepository::insert(store, &tangle).await.unwrap();

    let active = TangleRepository::list_active(store).await.unwrap();
    let found = active.iter().find(|t| t.id == tangle.id).unwrap();
    assert_eq!(found.task_ids, task_ids);
    assert_eq!(
        found.fingerprint,
        anamnesis_core::Fingerprint::of(&task_ids),
        "fingerprint is recomputed from the stored task_ids, not persisted directly"
    );

    let mut resolved = tangle.clone();
    resolved.resolved_at = Some(ts(7_500));
    TangleRepository::update(store, &resolved).await.unwrap();
    let active = TangleRepository::list_active(store).await.unwrap();
    assert!(
        !active.iter().any(|t| t.id == tangle.id),
        "a resolved tangle must no longer be active"
    );
}

// --- BoardQuery + suggestion_candidates + blocking_graph ---

async fn board_and_suggestion_contract(store: &SqlStore, project_id: ProjectId) {
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
    // Insert out of board-position order to prove ordering is read back
    // from `board_position`, not insertion order.
    TaskRepository::insert(store, &on_todo_first).await.unwrap();
    TaskRepository::insert(store, &on_todo_second)
        .await
        .unwrap();

    let mut blocker = task(project_id);
    blocker.placement = Placement::OnBoard {
        column: done.id,
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

    let count = BoardQuery::count_on_column(store, todo.id).await.unwrap();
    assert_eq!(count, 2);

    let state = BoardQuery::board_state(store, todo.id).await.unwrap();
    assert_eq!(state.wip_limit, Some(1));
    assert_eq!(state.current_count, 2);

    let columns = BoardQuery::columns_with_tasks(store).await.unwrap();
    let todo_column = columns.iter().find(|c| c.column.id == todo.id).unwrap();
    assert_eq!(
        todo_column.tasks.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![on_todo_second.id, on_todo_first.id],
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

    store.grant_system_admin(&admin).await.unwrap();
    assert!(
        MembershipQuery::is_system_admin(store, &admin)
            .await
            .unwrap()
    );
    assert!(
        !MembershipQuery::is_system_admin(store, &member)
            .await
            .unwrap()
    );

    store
        .set_area_role(&member, a.id, Role::Member)
        .await
        .unwrap();
    assert!(matches!(
        MembershipQuery::area_role(store, &member, a.id)
            .await
            .unwrap(),
        Some(Role::Member)
    ));
    assert_eq!(
        MembershipQuery::project_role(store, &member, p.id)
            .await
            .unwrap(),
        None,
        "an area role is not itself a project role"
    );

    // Inheritance: no explicit project role -> the area role applies.
    let effective = MembershipQuery::effective_role(store, &member, p.id, a.id)
        .await
        .unwrap();
    assert!(matches!(effective, Some(Role::Member)));

    // Strongest wins, not most specific: a *lower* explicit project role
    // does not demote the (higher) area role -- grants are independent and
    // stack, by analogy to `chmod` (adding a grant must never subtract
    // capability).
    store
        .set_area_role(&member, a.id, Role::ProjectAdmin)
        .await
        .unwrap();
    store
        .set_project_role(&member, p.id, Role::Member)
        .await
        .unwrap();
    let effective = MembershipQuery::effective_role(store, &member, p.id, a.id)
        .await
        .unwrap();
    assert!(
        matches!(effective, Some(Role::ProjectAdmin)),
        "a lower explicit project role must not demote a higher area role"
    );

    // And the reverse direction: a *higher* explicit project role still
    // elevates above a weaker area role.
    store
        .set_area_role(&member, a.id, Role::Member)
        .await
        .unwrap();
    store
        .set_project_role(&member, p.id, Role::ProjectAdmin)
        .await
        .unwrap();
    let effective = MembershipQuery::effective_role(store, &member, p.id, a.id)
        .await
        .unwrap();
    assert!(
        matches!(effective, Some(Role::ProjectAdmin)),
        "a higher explicit project role must still elevate above a lower area role"
    );

    // A stranger with no rows anywhere and no system admin grant has no
    // effective role at all.
    assert_eq!(
        MembershipQuery::effective_role(store, &stranger, p.id, a.id)
            .await
            .unwrap(),
        None
    );

    // System Admin resolves through the default `effective_area_role` /
    // `effective_role` even with no membership row anywhere.
    let admin_effective = MembershipQuery::effective_role(store, &admin, p.id, a.id)
        .await
        .unwrap();
    assert!(matches!(admin_effective, Some(Role::SystemAdmin)));
}

// --- Search: SearchIndex (write) + SearchQuery (read), across kinds ---

async fn search_contract(store: &SqlStore) {
    let area_id: anamnesis_core::AreaId = Uuid::new_v4().into();
    let project_id: ProjectId = Uuid::new_v4().into();
    let task_id: TaskId = Uuid::new_v4().into();

    // A distinctive whole word shared by all three titles -- see
    // `crate::sql::search`'s module doc comment on why this must be a whole
    // word, not a substring: FTS5/tsvector both match tokens.
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

    // Re-indexing an entity updates its title rather than duplicating it.
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
    assert!(hits.is_empty(), "removed entities must no longer be found");

    // A blank query returns no hits rather than every row.
    assert_eq!(SearchQuery::search(store, "").await.unwrap(), Vec::new());
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
         area_members, project_members, system_admins, projects, areas, board_columns \
         RESTART IDENTITY CASCADE",
    )
    .execute(&raw)
    .await
    .expect("truncate domain tables before the contract run");
    raw.close().await;

    contract(&store).await;
}
