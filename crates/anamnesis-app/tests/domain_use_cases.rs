//! Unit tests for the real-domain-model use cases in `anamnesis-app`
//! (`docs/DOMAIN.md`), driven against the in-memory fakes in
//! `domain_fakes`. Strict TDD from here on: every test below was written to
//! fail first, then made to pass by fixing or confirming the use case.

mod domain_fakes;
mod support;

use domain_fakes::Fakes;
use support::{FixedClock, SequentialIdGen};

// `anamnesis_app` and `anamnesis_core` both export same-named use-case /
// pure-transition functions (`create_area`, `create_project`, `create_task`,
// ...) — glob-importing both is ambiguous, so `anamnesis_app`'s use cases
// are globbed (that's what these tests exercise) and only the `anamnesis_core`
// *types* this file needs are imported explicitly, by name.
use anamnesis_app::*;
use anamnesis_core::policy::Role;
use anamnesis_core::{
    AreaId, Column, ColumnId, DomainError, FieldData, FieldKind, KindId, NumberValue, OfferItem,
    Outcome, Placement, ProjectId, ProjectStatus, SuggestionSettings, Task, TaskSummary, Title,
    UserId,
};

fn admin() -> Option<Role> {
    Some(Role::SystemAdmin)
}
fn project_admin() -> Option<Role> {
    Some(Role::ProjectAdmin)
}
fn member() -> Option<Role> {
    Some(Role::Member)
}
fn none() -> Option<Role> {
    None
}

fn alice() -> UserId {
    UserId::new("alice")
}
fn bob() -> UserId {
    UserId::new("bob")
}

// ============================= Areas =============================

#[tokio::test]
async fn create_area_requires_system_admin() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);

    let result = create_area(&fakes, &ids, &clock, &fakes, member(), "Home", "", 0).await;
    assert!(matches!(result, Err(AppError::Forbidden)));

    let area = create_area(&fakes, &ids, &clock, &fakes, admin(), "Home", "", 0)
        .await
        .unwrap();
    assert_eq!(area.title.as_str(), "Home");
}

#[tokio::test]
async fn list_areas_is_gated_and_ordered_by_position() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);

    create_area(&fakes, &ids, &clock, &fakes, admin(), "Work", "", 1)
        .await
        .unwrap();
    create_area(&fakes, &ids, &clock, &fakes, admin(), "Home", "", 0)
        .await
        .unwrap();

    assert!(matches!(
        list_areas(&fakes, none()).await,
        Err(AppError::Forbidden)
    ));

    let areas = list_areas(&fakes, admin()).await.unwrap();
    let titles: Vec<&str> = areas.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles, vec!["Home", "Work"]);
}

// ============================ Projects ============================

#[tokio::test]
async fn create_project_requires_project_admin_or_system_admin_in_the_area() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let area_id = AreaId::new(ids.next());

    let result = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        none(),
        area_id,
        "Renovate",
        "",
    )
    .await;
    assert!(matches!(result, Err(AppError::Forbidden)));

    // A plain Member of the Area is not enough -- creating a Project is
    // structural, gated the same as `EditProject`/`ManageFieldDefinitions`,
    // not the same as ordinary task work.
    let result = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        area_id,
        "Renovate",
        "",
    )
    .await;
    assert!(matches!(result, Err(AppError::Forbidden)));

    let ok = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area_id,
        "Renovate",
        "",
    )
    .await;
    assert!(ok.is_ok());

    let ok = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        admin(),
        area_id,
        "Renovate 2",
        "",
    )
    .await;
    assert!(ok.is_ok());
}

#[tokio::test]
async fn transition_project_status_enforces_the_active_project_limit() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let area_id = AreaId::new(ids.next());

    let p1 = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area_id,
        "One",
        "",
    )
    .await
    .unwrap();
    let p2 = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area_id,
        "Two",
        "",
    )
    .await
    .unwrap();

    transition_project_status(
        &fakes,
        &clock,
        project_admin(),
        p1.id,
        ProjectStatus::Active,
        1,
    )
    .await
    .unwrap();

    let result = transition_project_status(
        &fakes,
        &clock,
        project_admin(),
        p2.id,
        ProjectStatus::Active,
        1,
    )
    .await;
    assert_eq!(
        result,
        Err(AppError::Rule(DomainError::ActiveProjectLimitExceeded))
    );
}

#[tokio::test]
async fn transition_project_status_excludes_self_from_the_active_count() {
    // A project already Active, re-transitioned to Active again, must not
    // count itself twice against the limit.
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let area_id = AreaId::new(ids.next());

    let p1 = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area_id,
        "One",
        "",
    )
    .await
    .unwrap();
    transition_project_status(
        &fakes,
        &clock,
        project_admin(),
        p1.id,
        ProjectStatus::Active,
        1,
    )
    .await
    .unwrap();

    // Re-affirming Active -> Active must still succeed against a limit of 1.
    let result = transition_project_status(
        &fakes,
        &clock,
        project_admin(),
        p1.id,
        ProjectStatus::Active,
        1,
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn manage_field_definitions_requires_project_or_system_admin() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let area_id = AreaId::new(ids.next());
    let project = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area_id,
        "One",
        "",
    )
    .await
    .unwrap();

    let result = add_field_definition(
        &fakes,
        &ids,
        member(),
        project.id,
        "Priority",
        FieldKind::Number,
        0,
        true,
    )
    .await;
    assert!(matches!(result, Err(AppError::Forbidden)));

    let definition = add_field_definition(
        &fakes,
        &ids,
        project_admin(),
        project.id,
        "Priority",
        FieldKind::Number,
        0,
        true,
    )
    .await
    .unwrap();
    assert_eq!(definition.name.as_str(), "Priority");
}

// ============================== Tasks ==============================

fn some_project_id(ids: &SequentialIdGen) -> ProjectId {
    ProjectId::new(ids.next())
}

fn make_column(
    ids: &SequentialIdGen,
    title: &str,
    position: u32,
    wip_limit: Option<u32>,
    is_done: bool,
) -> Column {
    anamnesis_core::create_column(
        ColumnId::new(ids.next()),
        title,
        position,
        wip_limit,
        is_done,
    )
    .unwrap()
}

#[tokio::test]
async fn create_task_starts_below_the_horizon() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);

    let task = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Regrout",
        "",
    )
    .await
    .unwrap();
    assert_eq!(task.placement, Placement::Below);

    assert!(matches!(
        create_task(&fakes, &ids, &clock, &fakes, none(), project_id, "x", "").await,
        Err(AppError::Forbidden)
    ));
}

#[tokio::test]
async fn edit_task_uses_optimistic_concurrency_and_rejects_a_stale_write() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let task = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Regrout",
        "",
    )
    .await
    .unwrap();

    // Someone else edits it first, moving last_touched_at forward.
    let later = FixedClock::at(100);
    edit_task(
        &fakes,
        &later,
        &fakes,
        member(),
        task.id,
        "Regrout (v2)",
        "",
    )
    .await
    .unwrap();

    // Now a stale in-hand copy tries to write against the *original*
    // last_touched_at it was loaded with — the repository fake still holds
    // the same `task` value here because `edit_task` reloads internally, so
    // to actually exercise the conflict we drive it through the raw port.
    let stale_task = Task {
        title: Title::new("stale racer").unwrap(),
        ..task.clone()
    };
    let result = TaskRepository::update(&fakes, &stale_task, task.last_touched_at).await;
    assert!(matches!(result, Err(TaskUpdateError::Conflict)));
}

#[tokio::test]
async fn raise_task_is_refused_once_the_column_wip_limit_is_reached() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let column = make_column(&ids, "To-Do", 0, Some(1), false);
    fakes.seed_column(column.clone());

    let occupant = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Occupant",
        "",
    )
    .await
    .unwrap();
    raise_task(&fakes, &fakes, &clock, member(), occupant.id, column.id, 0)
        .await
        .unwrap();

    let newcomer = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Newcomer",
        "",
    )
    .await
    .unwrap();
    let result = raise_task(&fakes, &fakes, &clock, member(), newcomer.id, column.id, 0).await;
    assert!(matches!(result, Err(AppError::WipLimitExceeded)));
}

#[tokio::test]
async fn raise_task_allows_reordering_within_an_already_full_column() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let column = make_column(&ids, "To-Do", 0, Some(1), false);
    fakes.seed_column(column.clone());

    let occupant = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Occupant",
        "",
    )
    .await
    .unwrap();
    raise_task(&fakes, &fakes, &clock, member(), occupant.id, column.id, 0)
        .await
        .unwrap();

    // The column is now at its WIP limit of 1, but the *same* task moving to
    // a new position within it must not be treated as a new arrival.
    let result = raise_task(&fakes, &fakes, &clock, member(), occupant.id, column.id, 5).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn dropping_a_task_from_a_non_done_column_bounces_it() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let column = make_column(&ids, "Doing", 0, None, false);
    fakes.seed_column(column.clone());

    let task = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Task",
        "",
    )
    .await
    .unwrap();
    raise_task(&fakes, &fakes, &clock, member(), task.id, column.id, 0)
        .await
        .unwrap();

    let dropped = drop_task(&fakes, &clock, member(), task.id, false)
        .await
        .unwrap();
    assert_eq!(dropped.placement, Placement::Below);
    assert_eq!(dropped.bounce_count, 1);
    assert_eq!(dropped.last_bounced_at, Some(clock.now()));
}

#[tokio::test]
async fn dropping_a_task_from_a_done_column_does_not_bounce_it() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let done_column = make_column(&ids, "Done", 0, None, true);
    fakes.seed_column(done_column.clone());

    let task = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Task",
        "",
    )
    .await
    .unwrap();
    raise_task(&fakes, &fakes, &clock, member(), task.id, done_column.id, 0)
        .await
        .unwrap();

    let dropped = drop_task(&fakes, &clock, member(), task.id, true)
        .await
        .unwrap();
    assert_eq!(dropped.bounce_count, 0);
    assert_eq!(dropped.last_bounced_at, None);
}

#[tokio::test]
async fn dropping_a_task_that_is_already_below_the_horizon_does_not_bounce_it() {
    // Regression: a task that was never raised (still `Below` from
    // creation) must not accrue a bounce just because `drop_task` was
    // called on it — e.g. a double-submitted form.
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);

    let task = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Task",
        "",
    )
    .await
    .unwrap();
    assert_eq!(task.placement, Placement::Below);

    let dropped = drop_task(&fakes, &clock, member(), task.id, false)
        .await
        .unwrap();
    assert_eq!(dropped.placement, Placement::Below);
    assert_eq!(dropped.bounce_count, 0);
    assert_eq!(dropped.last_bounced_at, None);

    // Dropping it again (the double-submit case) must also stay a no-op.
    let dropped_again = drop_task(&fakes, &clock, member(), task.id, false)
        .await
        .unwrap();
    assert_eq!(dropped_again.bounce_count, 0);
    assert_eq!(dropped_again.last_bounced_at, None);
}

// ---- The deep-ancestor-walk risk: set_task_parent must walk the FULL chain ----

#[tokio::test]
async fn set_task_parent_rejects_a_cycle_that_closes_five_levels_up() {
    // Chain: root -> p1 -> p2 -> p3 -> leaf (leaf's parent is p3, p3's is
    // p2, p2's is p1, p1's is root; root has no parent). We then try to set
    // `root`'s parent to `leaf` — closing the cycle at the *far end* of a
    // five-task chain. A use case that only inspects the new parent's
    // immediate parent (rather than walking the whole chain) would see only
    // "leaf's parent is p3" and let this through; the real chain shows
    // `root` sitting at the top of `leaf`'s ancestry, so it must be rejected.
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);

    async fn task(
        fakes: &Fakes,
        ids: &SequentialIdGen,
        clock: &FixedClock,
        project_id: ProjectId,
        name: &str,
    ) -> Task {
        create_task(
            fakes,
            ids,
            clock,
            fakes,
            Some(Role::Member),
            project_id,
            name,
            "",
        )
        .await
        .unwrap()
    }

    let root = task(&fakes, &ids, &clock, project_id, "root").await;
    let p1 = task(&fakes, &ids, &clock, project_id, "p1").await;
    let p2 = task(&fakes, &ids, &clock, project_id, "p2").await;
    let p3 = task(&fakes, &ids, &clock, project_id, "p3").await;
    let leaf = task(&fakes, &ids, &clock, project_id, "leaf").await;

    set_task_parent(&fakes, &clock, member(), p1.id, Some(root.id))
        .await
        .unwrap();
    set_task_parent(&fakes, &clock, member(), p2.id, Some(p1.id))
        .await
        .unwrap();
    set_task_parent(&fakes, &clock, member(), p3.id, Some(p2.id))
        .await
        .unwrap();
    set_task_parent(&fakes, &clock, member(), leaf.id, Some(p3.id))
        .await
        .unwrap();

    // root -> parent = leaf would close a cycle 5 hops away.
    let result = set_task_parent(&fakes, &clock, member(), root.id, Some(leaf.id)).await;
    assert_eq!(result, Err(AppError::Rule(DomainError::ContainmentCycle)));

    // Sanity: root's parent must remain untouched by the rejected attempt.
    assert_eq!(fakes.task(root.id).parent_task_id, None);
}

#[tokio::test]
async fn set_task_parent_allows_a_non_cyclic_deep_reparent() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);

    async fn task(
        fakes: &Fakes,
        ids: &SequentialIdGen,
        clock: &FixedClock,
        project_id: ProjectId,
        name: &str,
    ) -> Task {
        create_task(
            fakes,
            ids,
            clock,
            fakes,
            Some(Role::Member),
            project_id,
            name,
            "",
        )
        .await
        .unwrap()
    }

    let a = task(&fakes, &ids, &clock, project_id, "a").await;
    let b = task(&fakes, &ids, &clock, project_id, "b").await;
    let c = task(&fakes, &ids, &clock, project_id, "c").await;
    // Unrelated task, not on any chain with a/b/c.
    let other = task(&fakes, &ids, &clock, project_id, "other").await;

    set_task_parent(&fakes, &clock, member(), b.id, Some(a.id))
        .await
        .unwrap();
    set_task_parent(&fakes, &clock, member(), c.id, Some(b.id))
        .await
        .unwrap();

    let result = set_task_parent(&fakes, &clock, member(), other.id, Some(c.id)).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn set_task_parent_terminates_against_an_already_corrupted_ancestor_chain() {
    // Defensive case, not reachable through `set_task_parent` itself (which
    // never lets a cycle get written): if the stored data were ever
    // corrupted into a self-referencing loop, `walk_ancestors` must still
    // terminate rather than looping forever. Wrapped in a timeout so a
    // regression fails this test rather than hanging the whole suite.
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);

    let a = create_task(&fakes, &ids, &clock, &fakes, member(), project_id, "a", "")
        .await
        .unwrap();
    let b = create_task(&fakes, &ids, &clock, &fakes, member(), project_id, "b", "")
        .await
        .unwrap();
    // Bypass the use case entirely to corrupt the chain: a's parent is b,
    // b's parent is a.
    fakes.seed_task(Task {
        parent_task_id: Some(b.id),
        ..a.clone()
    });
    fakes.seed_task(Task {
        parent_task_id: Some(a.id),
        ..b.clone()
    });

    let c = create_task(&fakes, &ids, &clock, &fakes, member(), project_id, "c", "")
        .await
        .unwrap();
    // No timeout wrapper needed here (the crate's `tokio` dev-dependency
    // does not enable the `time` feature): `walk_ancestors`'s `seen` guard
    // means this either returns promptly or the test genuinely hangs, which
    // would already fail the suite loudly enough to diagnose.
    let result = set_task_parent(&fakes, &clock, member(), c.id, Some(a.id)).await;
    assert!(result.is_ok());
}

// ============================ Suggestions ============================

fn settings() -> SuggestionSettings {
    SuggestionSettings {
        cooldown_seconds: 3600,
        high_bounce_threshold: 3,
    }
}

#[tokio::test]
async fn derive_seed_is_stable_for_an_unchanged_board_and_changes_when_it_changes() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let mut project =
        anamnesis_core::create_project(project_id, AreaId::new(ids.next()), "P", "", clock.now())
            .unwrap();
    project.status = ProjectStatus::Active;
    fakes.seed_project(project);

    let t1 = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "One",
        "",
    )
    .await
    .unwrap();

    let candidates_before = fakes.suggestion_candidates_for_test().await;
    let seed_before_1 = derive_seed(&alice(), (2026, 100), &candidates_before);
    let seed_before_2 = derive_seed(&alice(), (2026, 100), &candidates_before);
    assert_eq!(
        seed_before_1, seed_before_2,
        "an unchanged board must derive the identical seed (page refresh must not re-roll)"
    );

    // Now change the board: add a second task. The candidate set differs,
    // so the fingerprint -- and thus the seed -- must differ too.
    let _t2 = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Two",
        "",
    )
    .await
    .unwrap();
    let candidates_after = fakes.suggestion_candidates_for_test().await;
    let seed_after = derive_seed(&alice(), (2026, 100), &candidates_after);
    assert_ne!(
        seed_after, seed_before_1,
        "a changed board must derive a different seed"
    );

    // Also changes if the *same* board is queried on a different local date.
    let seed_different_date = derive_seed(&alice(), (2026, 101), &candidates_before);
    assert_ne!(seed_different_date, seed_before_1);

    // Also changes for a different user, same board and date.
    let seed_different_user = derive_seed(&bob(), (2026, 100), &candidates_before);
    assert_ne!(seed_different_user, seed_before_1);

    let _ = t1;
}

#[tokio::test]
async fn request_suggestion_stamps_last_offered_at_on_every_offered_task() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let area_id = AreaId::new(ids.next());
    let project = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area_id,
        "P",
        "",
    )
    .await
    .unwrap();
    transition_project_status(
        &fakes,
        &clock,
        project_admin(),
        project.id,
        ProjectStatus::Active,
        10,
    )
    .await
    .unwrap();

    let task = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project.id,
        "Do the thing",
        "",
    )
    .await
    .unwrap();
    assert_eq!(task.last_offered_at, None);

    let column = make_column(&ids, "To-Do", 0, Some(3), false);
    fakes.seed_column(column.clone());

    let outcome = request_suggestion(
        &fakes,
        &fakes,
        &clock,
        member(),
        &alice(),
        (2026, 1),
        column.id,
        &settings(),
    )
    .await
    .unwrap();

    match outcome {
        Outcome::Offer(offer) => assert!(!offer.items.is_empty()),
        other => panic!("expected an Offer, got {other:?}"),
    }
    assert_eq!(fakes.task(task.id).last_offered_at, Some(clock.now()));
}

#[tokio::test]
async fn request_suggestion_is_full_silence_at_the_wip_limit() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let area_id = AreaId::new(ids.next());
    let project = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area_id,
        "P",
        "",
    )
    .await
    .unwrap();
    transition_project_status(
        &fakes,
        &clock,
        project_admin(),
        project.id,
        ProjectStatus::Active,
        10,
    )
    .await
    .unwrap();

    let column = make_column(&ids, "To-Do", 0, Some(1), false);
    fakes.seed_column(column.clone());
    let occupant = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project.id,
        "Occupant",
        "",
    )
    .await
    .unwrap();
    raise_task(&fakes, &fakes, &clock, member(), occupant.id, column.id, 0)
        .await
        .unwrap();

    let _below = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project.id,
        "Below",
        "",
    )
    .await
    .unwrap();

    let outcome = request_suggestion(
        &fakes,
        &fakes,
        &clock,
        member(),
        &alice(),
        (2026, 1),
        column.id,
        &settings(),
    )
    .await
    .unwrap();
    assert_eq!(outcome, Outcome::Full);
}

// small helper trait extension for the seed test above, since
// `BoardQuery::suggestion_candidates` is async and this file has no direct
// access to it without importing the trait.
#[async_trait::async_trait]
trait SuggestionCandidatesForTest {
    async fn suggestion_candidates_for_test(&self) -> Vec<TaskSummary>;
}

#[async_trait::async_trait]
impl SuggestionCandidatesForTest for Fakes {
    async fn suggestion_candidates_for_test(&self) -> Vec<TaskSummary> {
        BoardQuery::suggestion_candidates(self).await.unwrap()
    }
}

// ============================== Tangles ==============================

#[tokio::test]
async fn run_tangle_detection_surfaces_a_knot_and_resolves_it() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);

    let a = create_task(&fakes, &ids, &clock, &fakes, member(), project_id, "A", "")
        .await
        .unwrap();
    let b = create_task(&fakes, &ids, &clock, &fakes, member(), project_id, "B", "")
        .await
        .unwrap();

    let a_blocks_b = create_relationship(
        &fakes,
        &fakes,
        &ids,
        &clock,
        member(),
        a.id,
        project_id,
        b.id,
        project_id,
        KindId::BUILTIN_BLOCKS,
    )
    .await
    .unwrap();
    create_relationship(
        &fakes,
        &fakes,
        &ids,
        &clock,
        member(),
        b.id,
        project_id,
        a.id,
        project_id,
        KindId::BUILTIN_BLOCKS,
    )
    .await
    .unwrap();

    let reconciliation = run_tangle_detection(&fakes, &fakes, &ids, &clock)
        .await
        .unwrap();
    assert_eq!(reconciliation.newly_detected.len(), 1);
    let tangle = &reconciliation.newly_detected[0];
    assert_eq!(tangle.task_ids, [a.id, b.id].into_iter().collect());

    // Break the block; detection must resolve it.
    delete_relationship(&fakes, member(), a_blocks_b.id)
        .await
        .unwrap();
    let reconciliation2 = run_tangle_detection(&fakes, &fakes, &ids, &clock)
        .await
        .unwrap();
    assert_eq!(reconciliation2.resolved.len(), 1);
    assert!(reconciliation2.resolved[0].resolved_at.is_some());
}

/// Builds a knotted pair (`a` blocks `b`, `b` blocks `a`) in a real, Active
/// project and runs detection once, returning `(project_id, a, b, tangle)`
/// — the shared setup every placement test below starts from. A *real*
/// Active project (not a bare `some_project_id`) matters here because the
/// suggestion-exclusion test needs `ProjectStatus::Active`, which the fakes'
/// `suggestion_candidates`/`blocking_graph` can only see for a project that
/// actually exists in the store.
async fn knotted_pair(
    fakes: &Fakes,
    ids: &SequentialIdGen,
    clock: &FixedClock,
) -> (ProjectId, Task, Task, anamnesis_core::Tangle) {
    let area_id = AreaId::new(ids.next());
    let project = create_project(fakes, ids, clock, fakes, project_admin(), area_id, "P", "")
        .await
        .unwrap();
    transition_project_status(
        fakes,
        clock,
        project_admin(),
        project.id,
        ProjectStatus::Active,
        10,
    )
    .await
    .unwrap();
    let project_id = project.id;
    let a = create_task(fakes, ids, clock, fakes, member(), project_id, "A", "")
        .await
        .unwrap();
    let b = create_task(fakes, ids, clock, fakes, member(), project_id, "B", "")
        .await
        .unwrap();
    create_relationship(
        fakes,
        fakes,
        ids,
        clock,
        member(),
        a.id,
        project_id,
        b.id,
        project_id,
        KindId::BUILTIN_BLOCKS,
    )
    .await
    .unwrap();
    create_relationship(
        fakes,
        fakes,
        ids,
        clock,
        member(),
        b.id,
        project_id,
        a.id,
        project_id,
        KindId::BUILTIN_BLOCKS,
    )
    .await
    .unwrap();
    let reconciliation = run_tangle_detection(fakes, fakes, ids, clock)
        .await
        .unwrap();
    let tangle = reconciliation.newly_detected[0].clone();
    (project_id, a, b, tangle)
}

#[tokio::test]
async fn place_tangle_puts_it_on_the_board_and_freezes_it() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let (_, _, _, tangle) = knotted_pair(&fakes, &ids, &clock).await;
    assert_eq!(tangle.placement, Placement::Below);
    assert!(!tangle.frozen);

    let column = make_column(&ids, "To-Do", 0, None, false);
    fakes.seed_column(column.clone());

    let placed = place_tangle(&fakes, &fakes, member(), tangle.id, column.id)
        .await
        .unwrap();
    assert_eq!(
        placed.placement,
        Placement::OnBoard {
            column: column.id,
            position: 0
        }
    );
    assert!(placed.frozen);

    let reloaded = TangleRepository::load(&fakes, tangle.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.placement, placed.placement);
    assert!(reloaded.frozen);
}

#[tokio::test]
async fn place_tangle_is_forbidden_with_no_role() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let (_, _, _, tangle) = knotted_pair(&fakes, &ids, &clock).await;
    let column = make_column(&ids, "To-Do", 0, None, false);
    fakes.seed_column(column.clone());

    let result = place_tangle(&fakes, &fakes, none(), tangle.id, column.id).await;
    assert!(matches!(result, Err(AppError::Forbidden)));
}

#[tokio::test]
async fn a_task_and_a_tangle_together_fill_a_wip_limit_of_two() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let column = make_column(&ids, "To-Do", 0, Some(2), false);
    fakes.seed_column(column.clone());

    let (project_id, _, _, tangle) = knotted_pair(&fakes, &ids, &clock).await;

    // One task raised: the column now holds 1 of 2.
    let occupant = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Occupant",
        "",
    )
    .await
    .unwrap();
    raise_task(&fakes, &fakes, &clock, member(), occupant.id, column.id, 0)
        .await
        .unwrap();

    // Placing the tangle fills the limit exactly: 1 task + 1 tangle == 2.
    place_tangle(&fakes, &fakes, member(), tangle.id, column.id)
        .await
        .unwrap();
    let state = BoardQuery::board_state(&fakes, column.id).await.unwrap();
    assert_eq!(
        state.current_count, 2,
        "a placed tangle must count against the column's WIP limit like a task"
    );

    // A third item -- another task -- is now refused: the tangle already
    // occupies the slot that would have gone to it.
    let newcomer = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Newcomer",
        "",
    )
    .await
    .unwrap();
    let result = raise_task(&fakes, &fakes, &clock, member(), newcomer.id, column.id, 1).await;
    assert!(matches!(result, Err(AppError::WipLimitExceeded)));
}

#[tokio::test]
async fn placing_a_tangle_is_itself_refused_once_the_column_is_already_at_its_wip_limit() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let column = make_column(&ids, "To-Do", 0, Some(1), false);
    fakes.seed_column(column.clone());
    let (project_id, _, _, tangle) = knotted_pair(&fakes, &ids, &clock).await;

    let occupant = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Occupant",
        "",
    )
    .await
    .unwrap();
    raise_task(&fakes, &fakes, &clock, member(), occupant.id, column.id, 0)
        .await
        .unwrap();

    let result = place_tangle(&fakes, &fakes, member(), tangle.id, column.id).await;
    assert!(matches!(result, Err(AppError::WipLimitExceeded)));
}

#[tokio::test]
async fn a_placed_tangle_keeps_its_board_slot_across_a_detection_pass() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let (_, _, _, tangle) = knotted_pair(&fakes, &ids, &clock).await;
    let column = make_column(&ids, "To-Do", 0, None, false);
    fakes.seed_column(column.clone());
    let placed = place_tangle(&fakes, &fakes, member(), tangle.id, column.id)
        .await
        .unwrap();

    // The exact same knot is detected again -- the ordinary "nothing
    // changed" re-run every board view does.
    let reconciliation = run_tangle_detection(&fakes, &fakes, &ids, &clock)
        .await
        .unwrap();
    assert!(reconciliation.newly_detected.is_empty());
    assert!(reconciliation.resolved.is_empty());
    assert_eq!(reconciliation.still_holding, vec![placed.clone()]);

    let reloaded = TangleRepository::load(&fakes, tangle.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.placement, placed.placement);
    assert!(reloaded.frozen);
}

#[tokio::test]
async fn drop_tangle_unfreezes_it_and_moves_it_below_the_horizon() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let (_, _, _, tangle) = knotted_pair(&fakes, &ids, &clock).await;
    let column = make_column(&ids, "To-Do", 0, None, false);
    fakes.seed_column(column.clone());
    place_tangle(&fakes, &fakes, member(), tangle.id, column.id)
        .await
        .unwrap();

    let dropped = drop_tangle(&fakes, member(), tangle.id).await.unwrap();
    assert_eq!(dropped.placement, Placement::Below);
    assert!(!dropped.frozen);
}

#[tokio::test]
async fn resolve_frozen_tangles_closes_one_and_moves_it_to_the_done_column() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let (_, a, b, tangle) = knotted_pair(&fakes, &ids, &clock).await;
    let todo = make_column(&ids, "To-Do", 0, None, false);
    let done = make_column(&ids, "Done", 1, None, true);
    fakes.seed_column(todo.clone());
    fakes.seed_column(done.clone());
    place_tangle(&fakes, &fakes, member(), tangle.id, todo.id)
        .await
        .unwrap();

    // Untangling: the edge b->a is removed, leaving only a->b -- no more
    // cycle in the frozen set.
    let b_blocks_a = RelationshipRepository::list_for_task(&fakes, b.id)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.from_task_id == b.id && r.to_task_id == a.id)
        .unwrap();
    delete_relationship(&fakes, member(), b_blocks_a.id)
        .await
        .unwrap();

    let resolved = resolve_frozen_tangles(&fakes, &fakes, &fakes, &clock, Some(done.id))
        .await
        .unwrap();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].id, tangle.id);
    assert!(resolved[0].resolved_at.is_some());
    assert_eq!(
        resolved[0].placement,
        Placement::OnBoard {
            column: done.id,
            position: 0
        },
        "a tangle that resolves while on the board moves into the is_done column"
    );

    let active = TangleRepository::list_active(&fakes).await.unwrap();
    assert!(!active.iter().any(|t| t.id == tangle.id));
}

#[tokio::test]
async fn a_tangle_already_on_the_board_is_not_offered_again_by_a_suggestion_request() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let (project_id, _, _, tangle) = knotted_pair(&fakes, &ids, &clock).await;
    let todo = make_column(&ids, "To-Do", 0, Some(3), false);
    fakes.seed_column(todo.clone());
    place_tangle(&fakes, &fakes, member(), tangle.id, todo.id)
        .await
        .unwrap();

    // A third, wholly unrelated eligible task in the same active project,
    // so the engine has something to offer -- proving specifically that the
    // *tangle* is excluded, not merely that everything happens to be stuck.
    create_task(&fakes, &ids, &clock, &fakes, member(), project_id, "C", "")
        .await
        .unwrap();

    let outcome = request_suggestion(
        &fakes,
        &fakes,
        &clock,
        member(),
        &alice(),
        (2026, 1),
        todo.id,
        &settings(),
    )
    .await
    .unwrap();
    let Outcome::Offer(offer) = outcome else {
        panic!("expected an Offer (task C is eligible), got {outcome:?}")
    };
    assert!(
        offer
            .items
            .iter()
            .all(|item| !matches!(item, OfferItem::Tangle(_))),
        "a tangle already on the board must never be offered again: {offer:?}"
    );
}

// ============================== Archive ==============================

#[tokio::test]
async fn archive_done_tasks_archives_everything_in_an_is_done_column() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let done = make_column(&ids, "Done", 0, None, true);
    fakes.seed_column(done.clone());

    let task = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Finished",
        "",
    )
    .await
    .unwrap();
    raise_task(&fakes, &fakes, &clock, member(), task.id, done.id, 0)
        .await
        .unwrap();

    let archived = archive_done_tasks(&fakes, &fakes, &fakes, &clock, &fakes, member())
        .await
        .unwrap();
    assert_eq!(archived.archived_task_ids, vec![task.id]);
    assert!(archived.archived_tangle_ids.is_empty());
    assert!(fakes.task(task.id).archived_at.is_some());
}

// --- Gap 2: a resolved tangle sitting in Done must be archived too, an
// unresolved one must not, and an archived tangle must never be resurrected
// by a later detection pass over the same (still-cyclic) tasks.

#[tokio::test]
async fn archive_done_tasks_archives_a_resolved_tangle_sitting_in_an_is_done_column() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let (_, a, b, tangle) = knotted_pair(&fakes, &ids, &clock).await;
    let todo = make_column(&ids, "To-Do", 0, None, false);
    let done = make_column(&ids, "Done", 1, None, true);
    fakes.seed_column(todo.clone());
    fakes.seed_column(done.clone());
    place_tangle(&fakes, &fakes, member(), tangle.id, todo.id)
        .await
        .unwrap();

    // Untangle it (break the cycle), then resolve it onto Done -- exactly
    // `resolve_frozen_tangles_closes_one_and_moves_it_to_the_done_column`'s
    // setup, since that is the only way a real tangle ever ends up resolved
    // and sitting in Done.
    let b_blocks_a = RelationshipRepository::list_for_task(&fakes, b.id)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.from_task_id == b.id && r.to_task_id == a.id)
        .unwrap();
    delete_relationship(&fakes, member(), b_blocks_a.id)
        .await
        .unwrap();
    resolve_frozen_tangles(&fakes, &fakes, &fakes, &clock, Some(done.id))
        .await
        .unwrap();

    let outcome = archive_done_tasks(&fakes, &fakes, &fakes, &clock, &fakes, member())
        .await
        .unwrap();
    assert_eq!(outcome.archived_tangle_ids, vec![tangle.id]);

    let archived = TangleRepository::load(&fakes, tangle.id)
        .await
        .unwrap()
        .unwrap();
    assert!(archived.archived_at.is_some());

    // It must actually vanish from the board, not merely carry the flag.
    let columns = BoardQuery::columns_with_items(&fakes).await.unwrap();
    let done_column = columns.iter().find(|c| c.column.id == done.id).unwrap();
    assert!(
        done_column.items.is_empty(),
        "an archived tangle must vanish from its column: {:?}",
        done_column.items
    );
}

#[tokio::test]
async fn archive_done_tasks_does_not_archive_an_unresolved_tangle_sitting_in_an_is_done_column() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let (_, _, _, tangle) = knotted_pair(&fakes, &ids, &clock).await;
    let done = make_column(&ids, "Done", 0, None, true);
    fakes.seed_column(done.clone());
    // Placed directly in the Done column while the knot is still cyclic --
    // nothing stops a user from placing a tangle there, and it must not be
    // treated as archivable just because of where it happens to sit.
    place_tangle(&fakes, &fakes, member(), tangle.id, done.id)
        .await
        .unwrap();

    let outcome = archive_done_tasks(&fakes, &fakes, &fakes, &clock, &fakes, member())
        .await
        .unwrap();
    assert!(
        outcome.archived_tangle_ids.is_empty(),
        "an unresolved tangle must never be archived, even sitting in Done"
    );
    let reloaded = TangleRepository::load(&fakes, tangle.id)
        .await
        .unwrap()
        .unwrap();
    assert!(reloaded.archived_at.is_none());
}

#[tokio::test]
async fn an_archived_tangle_is_not_resurrected_by_a_fresh_detection_pass_over_the_same_tasks() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let (project_id, a, b, tangle) = knotted_pair(&fakes, &ids, &clock).await;
    let todo = make_column(&ids, "To-Do", 0, None, false);
    let done = make_column(&ids, "Done", 1, None, true);
    fakes.seed_column(todo.clone());
    fakes.seed_column(done.clone());
    place_tangle(&fakes, &fakes, member(), tangle.id, todo.id)
        .await
        .unwrap();

    let b_blocks_a = RelationshipRepository::list_for_task(&fakes, b.id)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.from_task_id == b.id && r.to_task_id == a.id)
        .unwrap();
    delete_relationship(&fakes, member(), b_blocks_a.id)
        .await
        .unwrap();
    resolve_frozen_tangles(&fakes, &fakes, &fakes, &clock, Some(done.id))
        .await
        .unwrap();
    let outcome = archive_done_tasks(&fakes, &fakes, &fakes, &clock, &fakes, member())
        .await
        .unwrap();
    assert_eq!(outcome.archived_tangle_ids, vec![tangle.id]);

    // The user re-ties the exact same knot: re-create the b->a edge over
    // the same two tasks.
    create_relationship(
        &fakes,
        &fakes,
        &ids,
        &clock,
        member(),
        b.id,
        project_id,
        a.id,
        project_id,
        KindId::BUILTIN_BLOCKS,
    )
    .await
    .unwrap();

    let reconciliation = run_tangle_detection(&fakes, &fakes, &ids, &clock)
        .await
        .unwrap();
    assert_eq!(
        reconciliation.newly_detected.len(),
        1,
        "the same knot recurring after the old tangle was archived must mint \
         a brand-new tangle, not be suppressed or silently ignored: {reconciliation:?}"
    );
    let fresh = &reconciliation.newly_detected[0];
    assert_ne!(
        fresh.id, tangle.id,
        "the new tangle must not reuse the archived tangle's identity"
    );
    assert_eq!(fresh.task_ids, tangle.task_ids);
    assert!(fresh.resolved_at.is_none());
    assert!(fresh.archived_at.is_none());
}

// ============================ Search index ============================
//
// The Phase F2 regression: `SearchIndex` was called from the web handlers,
// not from these use cases, so any caller that only ever goes through
// `anamnesis-app` — a future MCP server or CLI, or these very tests — wrote
// an area/project/task that global search could never find. These are the
// tests that would have caught it: every one of them drives a use case
// alone, with no web layer involved, and then asserts against
// `SearchQuery::search` (`domain_fakes::Fakes` implements both the write
// side, `SearchIndex`, and the read side, `SearchQuery`, against one shared
// backing store — see that module's doc comment).

#[tokio::test]
async fn creating_an_area_through_the_use_case_alone_makes_it_findable_via_search() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);

    create_area(&fakes, &ids, &clock, &fakes, admin(), "Homesteading", "", 0)
        .await
        .unwrap();

    let hits = fakes.search("Homesteading").await.unwrap();
    assert!(
        hits.iter()
            .any(|h| matches!(h, SearchHit::Area { title, .. } if title == "Homesteading")),
        "an area created through the use case alone must be findable via search: {hits:?}"
    );
}

#[tokio::test]
async fn creating_a_project_through_the_use_case_alone_makes_it_findable_via_search() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let area = create_area(&fakes, &ids, &clock, &fakes, admin(), "Home", "", 0)
        .await
        .unwrap();

    create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area.id,
        "Renovation",
        "",
    )
    .await
    .unwrap();

    let hits = fakes.search("Renovation").await.unwrap();
    assert!(
        hits.iter()
            .any(|h| matches!(h, SearchHit::Project { title, .. } if title == "Renovation")),
        "a project created through the use case alone must be findable via search: {hits:?}"
    );
}

#[tokio::test]
async fn creating_a_task_through_the_use_case_alone_makes_it_findable_via_search() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);

    create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Regrout the shower",
        "",
    )
    .await
    .unwrap();

    let hits = fakes.search("Regrout").await.unwrap();
    assert!(
        hits.iter()
            .any(|h| matches!(h, SearchHit::Task { title, .. } if title == "Regrout the shower")),
        "a task created through the use case alone must be findable via search: {hits:?}"
    );
}

#[tokio::test]
async fn editing_a_task_through_the_use_case_alone_updates_the_indexed_content() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let task = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Original title",
        "",
    )
    .await
    .unwrap();

    edit_task(
        &fakes,
        &clock,
        &fakes,
        member(),
        task.id,
        "Renamed title",
        "",
    )
    .await
    .unwrap();

    let stale = fakes.search("Original").await.unwrap();
    assert!(
        stale.is_empty(),
        "the pre-edit title must no longer be findable: {stale:?}"
    );
    let fresh = fakes.search("Renamed").await.unwrap();
    assert!(
        fresh
            .iter()
            .any(|h| matches!(h, SearchHit::Task { title, .. } if title == "Renamed title")),
        "the post-edit title must be findable: {fresh:?}"
    );
}

#[tokio::test]
async fn archiving_a_task_through_the_use_case_alone_removes_it_from_the_index() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let task = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Retired task",
        "",
    )
    .await
    .unwrap();
    assert!(!fakes.search("Retired").await.unwrap().is_empty());

    archive_task(&fakes, &clock, &fakes, member(), task.id)
        .await
        .unwrap();

    let hits = fakes.search("Retired").await.unwrap();
    assert!(
        hits.is_empty(),
        "docs/DOMAIN.md §2: archived is vanished from every view unless \
         explicitly searched — an archived task must not surface in search: {hits:?}"
    );
}

#[tokio::test]
async fn unarchiving_a_task_through_the_use_case_alone_reindexes_it() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let task = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Reprieved task",
        "",
    )
    .await
    .unwrap();
    archive_task(&fakes, &clock, &fakes, member(), task.id)
        .await
        .unwrap();
    assert!(fakes.search("Reprieved").await.unwrap().is_empty());

    unarchive_task(&fakes, &clock, &fakes, member(), task.id)
        .await
        .unwrap();

    let hits = fakes.search("Reprieved").await.unwrap();
    assert!(
        hits.iter()
            .any(|h| matches!(h, SearchHit::Task { title, .. } if title == "Reprieved task")),
        "restoring an archived task must make it findable via search again: {hits:?}"
    );
}

#[tokio::test]
async fn archiving_a_project_through_the_use_case_alone_removes_it_from_the_index() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let area = create_area(&fakes, &ids, &clock, &fakes, admin(), "Home", "", 0)
        .await
        .unwrap();
    let project = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area.id,
        "Sunsetting saga",
        "",
    )
    .await
    .unwrap();
    assert!(!fakes.search("Sunsetting").await.unwrap().is_empty());

    archive_project(&fakes, &clock, &fakes, project_admin(), project.id)
        .await
        .unwrap();

    let hits = fakes.search("Sunsetting").await.unwrap();
    assert!(
        hits.is_empty(),
        "an archived project must not surface in search: {hits:?}"
    );
}

#[tokio::test]
async fn unarchiving_a_project_through_the_use_case_alone_reindexes_it() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let area = create_area(&fakes, &ids, &clock, &fakes, admin(), "Home", "", 0)
        .await
        .unwrap();
    let project = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area.id,
        "Revived saga",
        "",
    )
    .await
    .unwrap();
    archive_project(&fakes, &clock, &fakes, project_admin(), project.id)
        .await
        .unwrap();
    assert!(fakes.search("Revived").await.unwrap().is_empty());

    unarchive_project(&fakes, &clock, &fakes, project_admin(), project.id)
        .await
        .unwrap();

    let hits = fakes.search("Revived").await.unwrap();
    assert!(
        hits.iter()
            .any(|h| matches!(h, SearchHit::Project { title, .. } if title == "Revived saga")),
        "restoring an archived project must make it findable via search again: {hits:?}"
    );
}

#[tokio::test]
async fn archive_done_tasks_removes_every_swept_task_from_the_index() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let done = make_column(&ids, "Done", 0, None, true);
    fakes.seed_column(done.clone());

    let task = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_id,
        "Swept away",
        "",
    )
    .await
    .unwrap();
    raise_task(&fakes, &fakes, &clock, member(), task.id, done.id, 0)
        .await
        .unwrap();
    assert!(!fakes.search("Swept").await.unwrap().is_empty());

    let archived = archive_done_tasks(&fakes, &fakes, &fakes, &clock, &fakes, member())
        .await
        .unwrap();
    assert_eq!(archived.archived_task_ids, vec![task.id]);

    let hits = fakes.search("Swept").await.unwrap();
    assert!(
        hits.is_empty(),
        "the scheduled/manual sweep archive path must also drop the task from \
         the index, not just the single-task archive_task path: {hits:?}"
    );
}

// ============================== Comments ==============================

#[tokio::test]
async fn a_member_can_edit_their_own_comment_but_not_someone_elses() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let task = create_task(&fakes, &ids, &clock, &fakes, member(), project_id, "T", "")
        .await
        .unwrap();

    let comment = add_comment(&fakes, &ids, &clock, member(), task.id, alice(), "first")
        .await
        .unwrap();

    let own_edit = edit_comment_uc(&fakes, &clock, member(), &alice(), comment.id, "edited").await;
    assert!(own_edit.is_ok());

    let others_edit = edit_comment_uc(&fakes, &clock, member(), &bob(), comment.id, "hijack").await;
    assert!(matches!(others_edit, Err(AppError::Forbidden)));
}

// `edit_comment` from `anamnesis_app` collides in name with the `entities`
// one re-exported as `edit_comment_entity`; the use case is what we want.
async fn edit_comment_uc(
    repo: &dyn CommentRepository,
    clock: &dyn Clock,
    role: Option<Role>,
    editor: &UserId,
    id: CommentId,
    body: &str,
) -> Result<Comment, AppError> {
    edit_comment(repo, clock, role, editor, id, body).await
}

#[tokio::test]
async fn list_comments_and_list_attachments_are_gated_read_paths() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let task = create_task(&fakes, &ids, &clock, &fakes, member(), project_id, "T", "")
        .await
        .unwrap();

    assert!(matches!(
        list_comments(&fakes, none(), task.id).await,
        Err(AppError::Forbidden)
    ));
    assert!(list_comments(&fakes, member(), task.id).await.is_ok());

    assert!(matches!(
        list_attachments(&fakes, none(), task.id).await,
        Err(AppError::Forbidden)
    ));
    assert!(list_attachments(&fakes, member(), task.id).await.is_ok());
}

#[tokio::test]
async fn delete_comment_is_allowed_for_author_or_admin_only() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let task = create_task(&fakes, &ids, &clock, &fakes, member(), project_id, "T", "")
        .await
        .unwrap();
    let comment = add_comment(&fakes, &ids, &clock, member(), task.id, alice(), "hi")
        .await
        .unwrap();

    assert!(matches!(
        delete_comment(&fakes, member(), &bob(), comment.id).await,
        Err(AppError::Forbidden)
    ));
    assert!(
        delete_comment(&fakes, project_admin(), &bob(), comment.id)
            .await
            .is_ok()
    );
}

// ============================ Read-path gating ============================
// docs/DOMAIN.md's permission matrix must be enforced on read paths too, not
// only on writes -- a non-member must not be able to view a project, a
// task, or an area just because nothing there mutates state.

#[tokio::test]
async fn view_area_view_project_and_view_task_all_refuse_no_role() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);

    let area = create_area(&fakes, &ids, &clock, &fakes, admin(), "Home", "", 0)
        .await
        .unwrap();
    assert!(matches!(
        view_area(&fakes, none(), area.id).await,
        Err(AppError::Forbidden)
    ));
    // Areas are a real membership scope now (crate::policy's module doc
    // comment): any assigned role -- Member included -- can view the area,
    // same as `ViewProject`. Only the total absence of a role is refused.
    assert!(view_area(&fakes, member(), area.id).await.is_ok());
    assert!(view_area(&fakes, admin(), area.id).await.is_ok());

    let project = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area.id,
        "P",
        "",
    )
    .await
    .unwrap();
    assert!(matches!(
        view_project(&fakes, none(), project.id).await,
        Err(AppError::Forbidden)
    ));
    assert!(view_project(&fakes, member(), project.id).await.is_ok());

    let task = create_task(&fakes, &ids, &clock, &fakes, member(), project.id, "T", "")
        .await
        .unwrap();
    assert!(matches!(
        view_task(&fakes, none(), task.id).await,
        Err(AppError::Forbidden)
    ));
    assert!(view_task(&fakes, member(), task.id).await.is_ok());
}

// ============================== Areas: edit/reposition ==============================

#[tokio::test]
async fn edit_area_and_reposition_area_require_area_admin_or_system_admin() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let area = create_area(&fakes, &ids, &clock, &fakes, admin(), "Home", "", 0)
        .await
        .unwrap();

    assert!(matches!(
        edit_area(&fakes, &clock, &fakes, member(), area.id, "Renamed", "").await,
        Err(AppError::Forbidden)
    ));
    let edited = edit_area(
        &fakes,
        &clock,
        &fakes,
        project_admin(),
        area.id,
        "Renamed",
        "",
    )
    .await
    .unwrap();
    assert_eq!(edited.title.as_str(), "Renamed");

    assert!(matches!(
        reposition_area(&fakes, member(), area.id, 5).await,
        Err(AppError::Forbidden)
    ));
    let moved = reposition_area(&fakes, admin(), area.id, 5).await.unwrap();
    assert_eq!(moved.position, 5);
}

// ============================== Projects: edit/archive ==============================

#[tokio::test]
async fn edit_project_archive_and_unarchive_require_project_admin() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let area_id = AreaId::new(ids.next());
    let project = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area_id,
        "P",
        "",
    )
    .await
    .unwrap();

    assert!(matches!(
        edit_project_fields(
            &fakes,
            &clock,
            &fakes,
            member(),
            project.id,
            Some("New"),
            None
        )
        .await,
        Err(AppError::Forbidden)
    ));
    let edited = edit_project_fields(
        &fakes,
        &clock,
        &fakes,
        project_admin(),
        project.id,
        Some("New"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(edited.title.as_str(), "New");
    // description left unchanged (None passed through).
    assert_eq!(edited.description, "");

    assert!(matches!(
        archive_project(&fakes, &clock, &fakes, member(), project.id).await,
        Err(AppError::Forbidden)
    ));
    let archived = archive_project(&fakes, &clock, &fakes, project_admin(), project.id)
        .await
        .unwrap();
    assert!(archived.archived_at.is_some());

    let restored = unarchive_project(&fakes, &clock, &fakes, project_admin(), project.id)
        .await
        .unwrap();
    assert!(restored.archived_at.is_none());
}

#[tokio::test]
async fn rename_field_definition_requires_project_admin() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let area_id = AreaId::new(ids.next());
    let project = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area_id,
        "P",
        "",
    )
    .await
    .unwrap();
    let definition = add_field_definition(
        &fakes,
        &ids,
        project_admin(),
        project.id,
        "Priority",
        FieldKind::Number,
        0,
        true,
    )
    .await
    .unwrap();

    assert!(matches!(
        rename_field_definition(&fakes, member(), project.id, definition.id, "Urgency").await,
        Err(AppError::Forbidden)
    ));
    let renamed = rename_field_definition(
        &fakes,
        project_admin(),
        project.id,
        definition.id,
        "Urgency",
    )
    .await
    .unwrap();
    assert_eq!(renamed.name.as_str(), "Urgency");
}

// ============================== Task fields ==============================

#[tokio::test]
async fn set_task_field_value_rejects_a_kind_mismatch() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let area_id = AreaId::new(ids.next());
    let project = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area_id,
        "P",
        "",
    )
    .await
    .unwrap();
    let definition = add_field_definition(
        &fakes,
        &ids,
        project_admin(),
        project.id,
        "Priority",
        FieldKind::Number,
        0,
        true,
    )
    .await
    .unwrap();
    let task = create_task(&fakes, &ids, &clock, &fakes, member(), project.id, "T", "")
        .await
        .unwrap();

    let mismatched = set_task_field_value(
        &fakes,
        member(),
        &definition,
        task.id,
        FieldData::Line("not a number".to_string()),
    )
    .await;
    assert!(matches!(
        mismatched,
        Err(AppError::Rule(DomainError::FieldKindMismatch(_)))
    ));

    let ok = set_task_field_value(
        &fakes,
        member(),
        &definition,
        task.id,
        FieldData::Number(NumberValue { units: 5, scale: 0 }),
    )
    .await;
    assert!(ok.is_ok());
}

#[tokio::test]
async fn set_checklist_position_reorders_a_task() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let task = create_task(&fakes, &ids, &clock, &fakes, member(), project_id, "T", "")
        .await
        .unwrap();

    assert!(matches!(
        set_checklist_position(&fakes, none(), task.id, 3).await,
        Err(AppError::Forbidden)
    ));
    let reordered = set_checklist_position(&fakes, member(), task.id, 3)
        .await
        .unwrap();
    assert_eq!(reordered.checklist_position, 3);
}

// ============================== Relationships ==============================

#[tokio::test]
async fn resolve_kind_recognises_all_three_builtins_and_falls_back_to_the_repository() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();

    assert_eq!(
        resolve_kind(&fakes, KindId::BUILTIN_BLOCKS)
            .await
            .unwrap()
            .id,
        KindId::BUILTIN_BLOCKS
    );
    assert_eq!(
        resolve_kind(&fakes, KindId::BUILTIN_RELATES_TO)
            .await
            .unwrap()
            .id,
        KindId::BUILTIN_RELATES_TO
    );
    assert_eq!(
        resolve_kind(&fakes, KindId::BUILTIN_DUPLICATES)
            .await
            .unwrap()
            .id,
        KindId::BUILTIN_DUPLICATES
    );

    let unknown = KindId::new(uuid::Uuid::from_u128(999));
    assert!(matches!(
        resolve_kind(&fakes, unknown).await,
        Err(AppError::NotFound)
    ));

    let area_id = AreaId::new(ids.next());
    let clock = FixedClock::at(0);
    let project = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area_id,
        "P",
        "",
    )
    .await
    .unwrap();
    let custom = add_relationship_kind(
        &fakes,
        &ids,
        project_admin(),
        project.id,
        "inspired by",
        "inspired",
    )
    .await
    .unwrap();
    assert_eq!(resolve_kind(&fakes, custom.id).await.unwrap().id, custom.id);
}

#[tokio::test]
async fn create_relationship_use_case_rejects_a_custom_kind_across_projects() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let area_id = AreaId::new(ids.next());
    let project_a = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area_id,
        "A",
        "",
    )
    .await
    .unwrap();
    let project_b = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area_id,
        "B",
        "",
    )
    .await
    .unwrap();
    let custom = add_relationship_kind(
        &fakes,
        &ids,
        project_admin(),
        project_a.id,
        "inspired by",
        "inspired",
    )
    .await
    .unwrap();

    let a = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_a.id,
        "A-task",
        "",
    )
    .await
    .unwrap();
    let b = create_task(
        &fakes,
        &ids,
        &clock,
        &fakes,
        member(),
        project_b.id,
        "B-task",
        "",
    )
    .await
    .unwrap();

    let result = create_relationship(
        &fakes,
        &fakes,
        &ids,
        &clock,
        member(),
        a.id,
        project_a.id,
        b.id,
        project_b.id,
        custom.id,
    )
    .await;
    assert_eq!(
        result,
        Err(AppError::Rule(DomainError::RelationshipKindNotAllowed))
    );

    // The built-in blocks kind is fine across the same pair of projects.
    let ok = create_relationship(
        &fakes,
        &fakes,
        &ids,
        &clock,
        member(),
        a.id,
        project_a.id,
        b.id,
        project_b.id,
        KindId::BUILTIN_BLOCKS,
    )
    .await;
    assert!(ok.is_ok());
}

#[tokio::test]
async fn delete_relationship_requires_a_role_and_removes_the_edge() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let a = create_task(&fakes, &ids, &clock, &fakes, member(), project_id, "A", "")
        .await
        .unwrap();
    let b = create_task(&fakes, &ids, &clock, &fakes, member(), project_id, "B", "")
        .await
        .unwrap();
    let relationship = create_relationship(
        &fakes,
        &fakes,
        &ids,
        &clock,
        member(),
        a.id,
        project_id,
        b.id,
        project_id,
        KindId::BUILTIN_RELATES_TO,
    )
    .await
    .unwrap();

    assert!(matches!(
        delete_relationship(&fakes, none(), relationship.id).await,
        Err(AppError::Forbidden)
    ));
    delete_relationship(&fakes, member(), relationship.id)
        .await
        .unwrap();
    assert!(matches!(
        delete_relationship(&fakes, member(), relationship.id).await,
        Err(AppError::NotFound)
    ));
}

// ============================== Attachments ==============================

#[tokio::test]
async fn add_link_and_file_attachments_and_delete_cleans_up_the_blob() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let project_id = some_project_id(&ids);
    let task = create_task(&fakes, &ids, &clock, &fakes, member(), project_id, "T", "")
        .await
        .unwrap();

    assert!(matches!(
        add_link_attachment(&fakes, &ids, &clock, none(), task.id, "https://example.com").await,
        Err(AppError::Forbidden)
    ));
    let link = add_link_attachment(
        &fakes,
        &ids,
        &clock,
        member(),
        task.id,
        "https://example.com",
    )
    .await
    .unwrap();
    assert!(matches!(link.kind, AttachmentKind::Link { .. }));

    let file = add_file_attachment(
        &fakes,
        &fakes,
        &ids,
        &clock,
        member(),
        task.id,
        "photo.png",
        "image/png",
        vec![1, 2, 3],
    )
    .await
    .unwrap();
    let blob_key = match &file.kind {
        AttachmentKind::File { blob_key, .. } => blob_key.clone(),
        AttachmentKind::Link { .. } => panic!("expected a file attachment"),
    };
    assert!(BlobStore::get(&fakes, &blob_key).await.unwrap().is_some());

    let attachments = list_attachments(&fakes, member(), task.id).await.unwrap();
    assert_eq!(attachments.len(), 2);

    delete_attachment(&fakes, &fakes, member(), file.id)
        .await
        .unwrap();
    assert!(BlobStore::get(&fakes, &blob_key).await.unwrap().is_none());
}

// ============================ MembershipQuery ============================
// `effective_role`'s default-method composition (crate::ports::membership's
// module doc comment: "this is what every project-scoped use case should
// call"): System Admin, an Area grant, and a Project grant are independent
// -- `effective_role` takes the *strongest* of the three, never the most
// specific. Adding a further grant must never subtract capability.

fn some_area_id(ids: &SequentialIdGen) -> AreaId {
    AreaId::new(ids.next())
}

#[tokio::test]
async fn effective_role_prefers_system_admin_over_any_stored_project_role() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let area_id = some_area_id(&ids);
    let project_id = ProjectId::new(ids.next());
    let sam = UserId::new("sam");

    // Sam holds no project-local role row at all.
    assert_eq!(
        MembershipQuery::project_role(&fakes, &sam, project_id)
            .await
            .unwrap(),
        None
    );

    fakes.make_system_admin(&sam);
    assert_eq!(
        MembershipQuery::effective_role(&fakes, &sam, project_id, area_id)
            .await
            .unwrap(),
        Some(Role::SystemAdmin)
    );
}

#[tokio::test]
async fn effective_role_falls_through_to_the_stored_project_role() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let area_id = some_area_id(&ids);
    let project_id = ProjectId::new(ids.next());
    let priya = UserId::new("priya");

    assert_eq!(
        MembershipQuery::effective_role(&fakes, &priya, project_id, area_id)
            .await
            .unwrap(),
        None
    );

    fakes.set_project_role(&priya, project_id, Role::ProjectAdmin);
    assert_eq!(
        MembershipQuery::effective_role(&fakes, &priya, project_id, area_id)
            .await
            .unwrap(),
        Some(Role::ProjectAdmin)
    );
}

// ------------------------- Area-scoped inheritance -------------------------
// The project owner's fix for the Phase D gap: Areas are a real membership
// scope, and a Project inherits its Area's role when it carries no explicit
// project role of its own. Each test below is built to fail if inheritance
// were wired naively (see the Phase D report for what was falsified).
//
// A later correction: composition across scopes is "strongest grant wins",
// not "most specific scope wins" -- see the tests below named around
// "demote" and the `adding_a_further_grant_never_reduces_the_effective_role`
// monotonicity test (see the role-composition-fix report for what was
// falsified there).

#[tokio::test]
async fn a_role_held_only_on_the_area_is_inherited_by_a_project_with_no_role_of_its_own() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let area_id = some_area_id(&ids);
    let project_id = ProjectId::new(ids.next());
    let priya = UserId::new("priya");

    // Priya holds a role on the Area only -- nothing project-local at all.
    fakes.set_area_role(&priya, area_id, Role::ProjectAdmin);
    assert_eq!(
        MembershipQuery::project_role(&fakes, &priya, project_id)
            .await
            .unwrap(),
        None
    );

    assert_eq!(
        MembershipQuery::effective_role(&fakes, &priya, project_id, area_id)
            .await
            .unwrap(),
        Some(Role::ProjectAdmin)
    );
}

#[tokio::test]
async fn a_lower_explicit_project_role_does_not_demote_a_stronger_inherited_area_role() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let area_id = some_area_id(&ids);
    let project_id = ProjectId::new(ids.next());
    let priya = UserId::new("priya");

    // Priya administers the whole Area...
    fakes.set_area_role(&priya, area_id, Role::ProjectAdmin);
    // ...and is also explicitly added as a Member on this one project.
    fakes.set_project_role(&priya, project_id, Role::Member);

    // The grants are independent and stack -- adding the (lower) Member
    // grant on the project must not subtract the (higher) Area grant.
    // "Most specific scope wins" was rejected precisely because it did:
    // this is the defect the project owner asked to be fixed.
    assert_eq!(
        MembershipQuery::effective_role(&fakes, &priya, project_id, area_id)
            .await
            .unwrap(),
        Some(Role::ProjectAdmin)
    );
}

#[tokio::test]
async fn a_higher_explicit_project_role_elevates_above_a_weaker_inherited_area_role() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let area_id = some_area_id(&ids);
    let project_id = ProjectId::new(ids.next());
    let bob = UserId::new("bob");

    fakes.set_area_role(&bob, area_id, Role::Member);
    fakes.set_project_role(&bob, project_id, Role::ProjectAdmin);

    assert_eq!(
        MembershipQuery::effective_role(&fakes, &bob, project_id, area_id)
            .await
            .unwrap(),
        Some(Role::ProjectAdmin)
    );
}

#[tokio::test]
async fn adding_a_further_grant_never_reduces_the_effective_role() {
    // The actual property the project owner asked for, stated directly so a
    // future refactor can't quietly reintroduce "most specific scope wins"
    // while the individual before/after cases above still happen to pass.
    //
    // A starting grant state is any independent combination of (project
    // role, area role, system admin bit). "Adding a further grant" means
    // strengthening exactly one of those three slots to a role at least as
    // strong as whatever it already held (never a downgrade -- replacing an
    // explicit grant with a *weaker* one for the same scope is a deliberate
    // demotion, not "adding a grant", and is intentionally out of scope
    // here). For every such starting state and every such addition, the
    // effective role must never go down.
    let ids = SequentialIdGen::new();
    let area_id = some_area_id(&ids);
    let project_id = ProjectId::new(ids.next());

    // `None` first so `Option<Role>`'s derived `Ord` (`None` below every
    // `Some`) lines up with array order, letting `role_options[i..]` give
    // "at least as strong as `role_options[i]`" directly.
    let role_options: [Option<Role>; 4] = [
        None,
        Some(Role::Member),
        Some(Role::ProjectAdmin),
        Some(Role::SystemAdmin),
    ];

    async fn effective(
        fakes: &Fakes,
        user: &UserId,
        project_id: ProjectId,
        area_id: AreaId,
    ) -> Option<Role> {
        MembershipQuery::effective_role(fakes, user, project_id, area_id)
            .await
            .unwrap()
    }

    fn build(
        user: &UserId,
        project_id: ProjectId,
        area_id: AreaId,
        state: (Option<Role>, Option<Role>, bool),
    ) -> Fakes {
        let fakes = Fakes::new();
        let (project_role, area_role, is_admin) = state;
        if let Some(role) = project_role {
            fakes.set_project_role(user, project_id, role);
        }
        if let Some(role) = area_role {
            fakes.set_area_role(user, area_id, role);
        }
        if is_admin {
            fakes.make_system_admin(user);
        }
        fakes
    }

    for (p0_idx, &p0) in role_options.iter().enumerate() {
        for (a0_idx, &a0) in role_options.iter().enumerate() {
            for &s0 in &[false, true] {
                let user = UserId::new("monotonicity-probe");
                let before_fakes = build(&user, project_id, area_id, (p0, a0, s0));
                let before = effective(&before_fakes, &user, project_id, area_id).await;

                // Strengthen the project slot alone, to every role >= p0.
                for &p1 in &role_options[p0_idx..] {
                    let after_fakes = build(&user, project_id, area_id, (p1, a0, s0));
                    let after = effective(&after_fakes, &user, project_id, area_id).await;
                    assert!(
                        after >= before,
                        "strengthening the project grant from {p0:?} to {p1:?} (area={a0:?}, admin={s0}) \
                         reduced the effective role from {before:?} to {after:?}"
                    );
                }

                // Strengthen the area slot alone, to every role >= a0.
                for &a1 in &role_options[a0_idx..] {
                    let after_fakes = build(&user, project_id, area_id, (p0, a1, s0));
                    let after = effective(&after_fakes, &user, project_id, area_id).await;
                    assert!(
                        after >= before,
                        "strengthening the area grant from {a0:?} to {a1:?} (project={p0:?}, admin={s0}) \
                         reduced the effective role from {before:?} to {after:?}"
                    );
                }

                // Grant system admin (never revoke it -- s0 -> true is the
                // only direction "adding a further grant" can move).
                let after_fakes = build(&user, project_id, area_id, (p0, a0, true));
                let after = effective(&after_fakes, &user, project_id, area_id).await;
                assert!(
                    after >= before,
                    "granting system admin (project={p0:?}, area={a0:?}, was admin={s0}) \
                     reduced the effective role from {before:?} to {after:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn system_admin_overrides_everywhere_with_no_area_or_project_membership() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let area_id = some_area_id(&ids);
    let project_id = ProjectId::new(ids.next());
    let sam = UserId::new("sam");
    fakes.make_system_admin(&sam);

    assert_eq!(
        MembershipQuery::area_role(&fakes, &sam, area_id)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        MembershipQuery::project_role(&fakes, &sam, project_id)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        MembershipQuery::effective_area_role(&fakes, &sam, area_id)
            .await
            .unwrap(),
        Some(Role::SystemAdmin)
    );
    assert_eq!(
        MembershipQuery::effective_role(&fakes, &sam, project_id, area_id)
            .await
            .unwrap(),
        Some(Role::SystemAdmin)
    );
}

#[tokio::test]
async fn no_role_anywhere_resolves_to_none_on_area_and_project() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let area_id = some_area_id(&ids);
    let project_id = ProjectId::new(ids.next());
    let eve = UserId::new("eve");

    assert_eq!(
        MembershipQuery::effective_area_role(&fakes, &eve, area_id)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        MembershipQuery::effective_role(&fakes, &eve, project_id, area_id)
            .await
            .unwrap(),
        None
    );
}

// --------------------- Use-case level: Area role inheritance ---------------
// The same composition, exercised through the actual use cases (`view_area`,
// `create_project`, `view_project`) rather than the port directly.

#[tokio::test]
async fn a_user_with_only_an_area_role_can_view_that_area_and_create_a_project_in_it() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let priya = UserId::new("priya");

    let area = create_area(&fakes, &ids, &clock, &fakes, admin(), "Home", "", 0)
        .await
        .unwrap();
    fakes.set_area_role(&priya, area.id, Role::ProjectAdmin);

    let role = MembershipQuery::effective_area_role(&fakes, &priya, area.id)
        .await
        .unwrap();
    assert!(view_area(&fakes, role, area.id).await.is_ok());
    assert!(
        create_project(&fakes, &ids, &clock, &fakes, role, area.id, "Renovate", "")
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_user_with_only_a_project_role_cannot_see_a_sibling_project_or_the_area() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let bob = UserId::new("bob");

    let area = create_area(&fakes, &ids, &clock, &fakes, admin(), "Home", "", 0)
        .await
        .unwrap();
    let p1 = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area.id,
        "One",
        "",
    )
    .await
    .unwrap();
    let p2 = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area.id,
        "Two",
        "",
    )
    .await
    .unwrap();
    fakes.set_project_role(&bob, p1.id, Role::Member);

    // Bob can see the project he actually holds a role on...
    let role_on_p1 = MembershipQuery::effective_role(&fakes, &bob, p1.id, area.id)
        .await
        .unwrap();
    assert!(view_project(&fakes, role_on_p1, p1.id).await.is_ok());

    // ...but not its sibling in the same Area...
    let role_on_p2 = MembershipQuery::effective_role(&fakes, &bob, p2.id, area.id)
        .await
        .unwrap();
    assert!(matches!(
        view_project(&fakes, role_on_p2, p2.id).await,
        Err(AppError::Forbidden)
    ));

    // ...nor the Area itself.
    let area_role = MembershipQuery::effective_area_role(&fakes, &bob, area.id)
        .await
        .unwrap();
    assert!(matches!(
        view_area(&fakes, area_role, area.id).await,
        Err(AppError::Forbidden)
    ));
}

#[tokio::test]
async fn a_lower_explicit_project_role_does_not_demote_area_role_through_the_use_cases() {
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let priya = UserId::new("priya");

    let area = create_area(&fakes, &ids, &clock, &fakes, admin(), "Home", "", 0)
        .await
        .unwrap();
    let project = create_project(
        &fakes,
        &ids,
        &clock,
        &fakes,
        project_admin(),
        area.id,
        "One",
        "",
    )
    .await
    .unwrap();

    // Priya administers the whole Area...
    fakes.set_area_role(&priya, area.id, Role::ProjectAdmin);
    // ...and is also explicitly added as a Member on this one project.
    fakes.set_project_role(&priya, project.id, Role::Member);

    let role = MembershipQuery::effective_role(&fakes, &priya, project.id, area.id)
        .await
        .unwrap();
    assert_eq!(role, Some(Role::ProjectAdmin));

    // Priya can still manage field definitions: the Member grant on this
    // one project is an independent addition, not a demotion of her Area
    // Admin standing.
    let result = add_field_definition(
        &fakes,
        &ids,
        role,
        project.id,
        "Priority",
        FieldKind::Number,
        0,
        true,
    )
    .await;
    assert!(result.is_ok());
}

// ============================== Settings ==============================
//
// `view_settings`/`update_settings` (`crate::use_cases::settings`): the
// port-and-use-case half of the runtime settings gap. Both directions are
// System-Admin-only, including the read path — see that module's doc
// comment for why there is no `Action::ViewSettings` open to a Member the
// way `view_area`/`view_project`/`view_task` are.

#[tokio::test]
async fn view_settings_requires_system_admin() {
    let fakes = Fakes::new();

    assert!(matches!(
        view_settings(&fakes, none()).await,
        Err(AppError::Forbidden)
    ));
    assert!(matches!(
        view_settings(&fakes, member()).await,
        Err(AppError::Forbidden)
    ));
    assert!(matches!(
        view_settings(&fakes, project_admin()).await,
        Err(AppError::Forbidden)
    ));

    let settings = view_settings(&fakes, admin()).await.unwrap();
    assert_eq!(settings.active_project_limit, DEFAULT_ACTIVE_PROJECT_LIMIT);
}

#[tokio::test]
async fn update_settings_requires_system_admin() {
    let fakes = Fakes::new();
    let new_settings = Settings {
        active_project_limit: 9,
        ..Settings::default()
    };

    assert!(matches!(
        update_settings(&fakes, none(), new_settings).await,
        Err(AppError::Forbidden)
    ));
    assert!(matches!(
        update_settings(&fakes, member(), new_settings).await,
        Err(AppError::Forbidden)
    ));
    assert!(matches!(
        update_settings(&fakes, project_admin(), new_settings).await,
        Err(AppError::Forbidden)
    ));

    let stored = update_settings(&fakes, admin(), new_settings)
        .await
        .unwrap();
    assert_eq!(stored.active_project_limit, 9);
}

#[tokio::test]
async fn update_settings_changes_the_active_project_limit_that_transition_status_enforces() {
    // The point of a `SettingsRepository` at all: editing the stored value
    // must actually change what `transition_project_status` enforces, not
    // just what `view_settings` echoes back.
    let fakes = Fakes::new();
    let ids = SequentialIdGen::new();
    let clock = FixedClock::at(0);
    let area = create_area(&fakes, &ids, &clock, &fakes, admin(), "Home", "", 0)
        .await
        .unwrap();
    let one = create_project(&fakes, &ids, &clock, &fakes, admin(), area.id, "One", "")
        .await
        .unwrap();
    let two = create_project(&fakes, &ids, &clock, &fakes, admin(), area.id, "Two", "")
        .await
        .unwrap();

    update_settings(
        &fakes,
        admin(),
        Settings {
            active_project_limit: 1,
            ..Settings::default()
        },
    )
    .await
    .unwrap();
    let limit = view_settings(&fakes, admin())
        .await
        .unwrap()
        .active_project_limit;

    transition_project_status(
        &fakes,
        &clock,
        admin(),
        one.id,
        ProjectStatus::Active,
        limit,
    )
    .await
    .unwrap();
    let result = transition_project_status(
        &fakes,
        &clock,
        admin(),
        two.id,
        ProjectStatus::Active,
        limit,
    )
    .await;
    assert!(matches!(
        result,
        Err(AppError::ActiveProjectLimitExceeded) | Err(AppError::Rule(_))
    ));
}

#[tokio::test]
async fn update_settings_never_touches_last_swept_at() {
    let fakes = Fakes::new();
    let swept_at = anamnesis_core::Timestamp::from_unix_seconds(1_000).unwrap();
    SettingsRepository::record_sweep(&fakes, swept_at)
        .await
        .unwrap();

    let stored = update_settings(
        &fakes,
        admin(),
        Settings {
            active_project_limit: 42,
            ..Settings::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(stored.active_project_limit, 42);
    assert_eq!(stored.last_swept_at, Some(swept_at));
}
