//! Steps for `tangles.feature`: SCC-based tangle detection and
//! reconciliation, exercised directly against the pure `anamnesis_core`
//! functions. No repository, no fakes -- these are pure, so there is
//! nothing to fake.

use cucumber::gherkin::Step;
use cucumber::{given, then, when};

use anamnesis_core::{Timestamp, builtin_blocks, detect_tangles, reconcile};

use super::AppWorld;

fn table_names(step: &Step) -> Vec<String> {
    let table = step.table.as_ref().expect("expected a data table");
    table.rows.iter().skip(1).map(|r| r[0].clone()).collect()
}

#[given(expr = "the following tasks exist:")]
async fn tasks_exist(world: &mut AppWorld, step: &Step) {
    for name in table_names(step) {
        world.core_task(&name);
    }
}

#[given(expr = "{string} blocks {string}")]
async fn declares_a_blocks_edge(world: &mut AppWorld, from: String, to: String) {
    world.add_blocks_edge(&from, &to);
}

#[when(expr = "{string} no longer blocks {string}")]
async fn breaks_a_blocks_edge(world: &mut AppWorld, from: String, to: String) {
    world.remove_blocks_edge(&from, &to);
}

/// Runs one detection + reconciliation pass: `detect_tangles` over the
/// scenario's current relationships, then `reconcile` against whatever is
/// currently in `world.stored_tangles` (empty on the first call). Backs all
/// three step phrasings ("detected", "detected again", "detected and
/// stored") -- they are the same action at different points in a scenario.
async fn detect_and_reconcile(world: &mut AppWorld) {
    let kinds = vec![builtin_blocks()];
    let detected = detect_tangles(&world.relationships, &kinds);
    world.last_detected = detected.clone();

    let now = Timestamp::from_unix_seconds(1_000).unwrap();
    // Reserve one fresh id per detected tangle -- an overestimate of how
    // many are actually *new*, which is exactly what `reconcile` expects: it
    // only draws from this as needed.
    let fresh_ids: Vec<_> = (0..detected.len())
        .map(|_| world.fresh_tangle_id())
        .collect();
    let reconciliation = reconcile(&detected, &world.stored_tangles, now, fresh_ids);

    // `stored_tangles` mirrors what a real system would persist: every
    // currently-active tangle, ready to be `previous` on the next pass.
    world.stored_tangles = reconciliation
        .newly_detected
        .iter()
        .chain(reconciliation.still_holding.iter())
        .cloned()
        .collect();
    world.last_reconciliation = Some(reconciliation);
}

#[given(expr = "tangles have been detected and stored")]
async fn given_detected_and_stored(world: &mut AppWorld) {
    detect_and_reconcile(world).await;
}

#[when(expr = "tangles are detected")]
#[when(expr = "tangles are detected again")]
async fn when_detected(world: &mut AppWorld) {
    detect_and_reconcile(world).await;
}

#[then(expr = "exactly one tangle is detected")]
async fn exactly_one_tangle_detected(world: &mut AppWorld) {
    assert_eq!(
        world.last_detected.len(),
        1,
        "expected exactly one tangle, got {:?}",
        world.last_detected
    );
}

#[then(expr = "the tangle contains the following tasks:")]
async fn tangle_contains(world: &mut AppWorld, step: &Step) {
    let names = table_names(step);
    let expected: std::collections::BTreeSet<_> =
        names.iter().map(|n| world.core_task(n)).collect();
    assert_eq!(world.last_detected.len(), 1, "expected exactly one tangle");
    assert_eq!(world.last_detected[0].task_ids, expected);
}

#[then(expr = "no tangle is active any longer")]
async fn no_tangle_active(world: &mut AppWorld) {
    assert!(
        world.stored_tangles.is_empty(),
        "expected no active tangle, got {:?}",
        world.stored_tangles
    );
}

#[then(expr = "the previously stored tangle is now marked resolved")]
async fn previously_stored_tangle_resolved(world: &mut AppWorld) {
    let reconciliation = world
        .last_reconciliation
        .as_ref()
        .expect("a detection pass must have run");
    assert_eq!(
        reconciliation.resolved.len(),
        1,
        "expected exactly one resolved tangle, got {:?}",
        reconciliation.resolved
    );
    assert!(reconciliation.resolved[0].resolved_at.is_some());
}
