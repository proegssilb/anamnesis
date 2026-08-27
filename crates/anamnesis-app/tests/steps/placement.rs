//! Steps for `placement.feature`: raising a task above the horizon and
//! dropping it back, including bounce accounting and the column WIP-limit
//! guard -- exercised through the real `raise_task`/`drop_task` use cases
//! against `domain_fakes::Fakes`.
//!
//! The `Given "X" is a Member of "Y"` / task-below-the-horizon steps this
//! feature relies on are registered once, in `access_control.rs`; Gherkin
//! step text is shared across `.feature` files by phrase, not by source
//! file, so re-declaring an identical pattern here would just make the
//! match ambiguous.

use cucumber::{given, then, when};

use anamnesis_app::{AppError, drop_task, raise_task};
use anamnesis_core::Placement;

use super::AppWorld;

#[given(regex = r#"^"([^"]+)" is a column with no work-in-progress limit that is not done$"#)]
async fn a_column_with_no_wip_limit_not_done(world: &mut AppWorld, column_name: String) {
    world.domain_column(&column_name, None, false);
}

#[given(regex = r#"^"([^"]+)" is a done column with no work-in-progress limit$"#)]
async fn a_done_column_with_no_wip_limit(world: &mut AppWorld, column_name: String) {
    world.domain_column(&column_name, None, true);
}

#[given(
    regex = r#"^"([^"]+)" is a column with a work-in-progress limit of (\d+) that is not done$"#
)]
async fn a_column_with_a_wip_limit(world: &mut AppWorld, column_name: String, limit: u32) {
    world.domain_column(&column_name, Some(limit), false);
}

#[when(regex = r#"^"([^"]+)" raises "([^"]+)" into "([^"]+)"$"#)]
async fn raises_into(world: &mut AppWorld, user: String, task_name: String, column_name: String) {
    let role = world.domain_role(&user);
    let task_id = world.domain_task_id(&task_name);
    let column_id = world.domain_column_id(&column_name);
    let result = raise_task(
        &world.domain,
        &world.domain,
        &world.clock,
        role,
        task_id,
        column_id,
        0,
    )
    .await;
    world.last_domain_error = result.err();
}

#[when(regex = r#"^"([^"]+)" drops "([^"]+)" back below the horizon without finishing it$"#)]
async fn drops_back_unfinished(world: &mut AppWorld, user: String, task_name: String) {
    let role = world.domain_role(&user);
    let task_id = world.domain_task_id(&task_name);
    let result = drop_task(&world.domain, &world.clock, role, task_id, false).await;
    world.last_domain_error = result.err();
}

#[when(regex = r#"^"([^"]+)" drops "([^"]+)" back below the horizon, finished$"#)]
async fn drops_back_finished(world: &mut AppWorld, user: String, task_name: String) {
    let role = world.domain_role(&user);
    let task_id = world.domain_task_id(&task_name);
    let result = drop_task(&world.domain, &world.clock, role, task_id, true).await;
    world.last_domain_error = result.err();
}

#[then(expr = "{string} is on the board")]
async fn is_on_the_board(world: &mut AppWorld, task_name: String) {
    let task = world.domain_task_state(&task_name);
    assert!(
        matches!(task.placement, Placement::OnBoard { .. }),
        "expected {task_name:?} to be on the board, got {:?}",
        task.placement
    );
}

#[then(expr = "{string} is below the horizon")]
async fn is_below_the_horizon(world: &mut AppWorld, task_name: String) {
    let task = world.domain_task_state(&task_name);
    assert_eq!(task.placement, Placement::Below);
}

#[then(regex = r#"^"([^"]+)" has bounced (\d+) times?$"#)]
async fn has_bounced_n_times(world: &mut AppWorld, task_name: String, times: u32) {
    let task = world.domain_task_state(&task_name);
    assert_eq!(
        task.bounce_count, times,
        "expected {task_name:?} to have bounced {times} time(s), got {}",
        task.bounce_count
    );
}

#[then(expr = "the move is refused because the column is at its work-in-progress limit")]
async fn move_refused_wip_limit(world: &mut AppWorld) {
    assert!(
        matches!(world.last_domain_error, Some(AppError::WipLimitExceeded)),
        "expected a WIP-limit refusal, got {:?}",
        world.last_domain_error
    );
}
