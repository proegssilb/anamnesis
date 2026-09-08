//! Steps for `task_lifecycle.feature`: capture, edit, archive/unarchive,
//! checklist parenting (and its acyclic guard), exercised through the real
//! `create_task`/`edit_task`/`archive_task`/`unarchive_task`/
//! `set_task_parent` use cases against `domain_fakes::Fakes`.

use cucumber::{given, then, when};

use anamnesis_app::{AppError, Clock, TaskRepository, archive_task, create_task, set_task_parent};
use anamnesis_core::Title;

use super::AppWorld;
use crate::support::FixedClock;

#[when(regex = r#"^"([^"]+)" captures a task "([^"]+)" in project "([^"]+)"$"#)]
async fn captures_a_task(
    world: &mut AppWorld,
    user: String,
    task_name: String,
    project_name: String,
) {
    let role = world.domain_role(&user);
    let project_id = world.domain_project(&project_name);
    let task = create_task(
        &world.domain,
        &world.ids,
        &world.clock,
        &world.domain,
        role,
        project_id,
        &task_name,
        "",
    )
    .await
    .expect("scenario setup: capturing the task must succeed");
    world.register_domain_task(&task_name, task.id);
}

#[when(regex = r#"^"([^"]+)" archives task "([^"]+)"$"#)]
async fn archives_task(world: &mut AppWorld, user: String, task_name: String) {
    let role = world.domain_role(&user);
    let task_id = world.domain_task_id(&task_name);
    let result = archive_task(&world.domain, &world.clock, &world.domain, role, task_id).await;
    world.last_domain_error = result.err();
}

#[when(regex = r#"^"([^"]+)" unarchives task "([^"]+)"$"#)]
async fn unarchives_task(world: &mut AppWorld, user: String, task_name: String) {
    let role = world.domain_role(&user);
    let task_id = world.domain_task_id(&task_name);
    let result =
        anamnesis_app::unarchive_task(&world.domain, &world.clock, &world.domain, role, task_id)
            .await;
    world.last_domain_error = result.err();
}

#[then(regex = r#"^task "([^"]+)" is archived$"#)]
async fn task_is_archived(world: &mut AppWorld, task_name: String) {
    let task = world.domain_task_state(&task_name);
    assert!(
        task.archived_at.is_some(),
        "expected {task_name:?} to be archived"
    );
}

#[then(regex = r#"^task "([^"]+)" is not archived$"#)]
async fn task_is_not_archived(world: &mut AppWorld, task_name: String) {
    let task = world.domain_task_state(&task_name);
    assert!(
        task.archived_at.is_none(),
        "expected {task_name:?} not to be archived"
    );
}

#[given(regex = r#"^"([^"]+)" is a checklist item of "([^"]+)"$"#)]
async fn is_a_checklist_item_of(world: &mut AppWorld, child: String, parent: String) {
    let parent_id = world.domain_task_id(&parent);
    // Scenario setup, not the behaviour under test -- see `world.rs`'s
    // `domain_task` doc comment for why setup goes straight through
    // `anamnesis_core` rather than the (permission-gated) use case.
    let child_task = world.domain_task_state(&child);
    let now = world.clock.now();
    let reparented = anamnesis_core::set_parent(&child_task, Some(parent_id), &[parent_id], now)
        .expect("scenario setup: a non-cyclic checklist reparent must succeed");
    world.domain.seed_task(reparented);
}

#[when(regex = r#"^"([^"]+)" (?:tries to make|makes) "([^"]+)" a checklist item of "([^"]+)"$"#)]
async fn makes_a_checklist_item_of(
    world: &mut AppWorld,
    user: String,
    child: String,
    parent: String,
) {
    let role = world.domain_role(&user);
    let child_id = world.domain_task_id(&child);
    let parent_id = world.domain_task_id(&parent);
    let result =
        set_task_parent(&world.domain, &world.clock, role, child_id, Some(parent_id)).await;
    world.last_domain_error = result.err();
}

#[then(regex = r#"^"([^"]+)" is contained in "([^"]+)"$"#)]
async fn is_contained_in(world: &mut AppWorld, child: String, parent: String) {
    let parent_id = world.domain_task_id(&parent);
    let task = world.domain_task_state(&child);
    assert_eq!(task.parent_task_id, Some(parent_id));
}

#[then(expr = "the reparenting is refused because it would contain its own ancestor")]
async fn reparenting_refused_cycle(world: &mut AppWorld) {
    assert!(
        matches!(
            world.last_domain_error,
            Some(AppError::Rule(
                anamnesis_core::DomainError::ContainmentCycle
            ))
        ),
        "expected a containment-cycle refusal, got {:?}",
        world.last_domain_error
    );
}

#[given(regex = r#"^"([^"]+)" has task "([^"]+)" open for editing$"#)]
async fn has_task_open_for_editing(world: &mut AppWorld, _user: String, task_name: String) {
    world.snapshot_domain_task(&task_name);
}

#[when(regex = r#"^"([^"]+)" edits task "([^"]+)" to have title "([^"]+)"$"#)]
async fn edits_task_to_have_title(
    world: &mut AppWorld,
    user: String,
    task_name: String,
    new_title: String,
) {
    let role = world.domain_role(&user);
    let task_id = world.domain_task_id(&task_name);
    // The fixed clock never advances on its own; a second writer's edit only
    // actually conflicts with a stale in-hand copy once the two disagree
    // about "when", so move it forward before this write lands.
    world.clock = FixedClock::at(100);
    let result = anamnesis_app::edit_task(
        &world.domain,
        &world.clock,
        &world.domain,
        role,
        task_id,
        &new_title,
        "",
    )
    .await;
    world.last_domain_error = result.err();
}

#[then(
    regex = r#"^"([^"]+)" saving her stale edit of "([^"]+)" is refused because it was concurrently modified$"#
)]
async fn saving_stale_edit_is_refused(world: &mut AppWorld, _user: String, task_name: String) {
    let snapshot = world.domain_task_snapshot(&task_name);
    let current = world.domain_task_state(&task_name);
    let stale = anamnesis_core::Task {
        title: Title::new("a stale racer's edit").unwrap(),
        ..current
    };
    let result = TaskRepository::update(&world.domain, &stale, snapshot).await;
    assert!(
        matches!(result, Err(anamnesis_app::TaskUpdateError::Conflict)),
        "expected the stale write to conflict, got {result:?}"
    );
}
