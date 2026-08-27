//! Steps for `suggestions.feature`: the suggestion engine, exercised
//! directly against the pure `anamnesis_core::suggest` function. No
//! repository, no fakes -- it is pure, so there is nothing to fake.

use cucumber::{given, then, when};

use anamnesis_core::{
    Blockage, BlockingGraph, BoardState, Offer, OfferItem, Outcome, Placement, ProjectStatus,
    SuggestionSettings, Timestamp, suggest,
};

use super::AppWorld;

#[given(expr = "a board with a work-in-progress limit of {int}, currently holding {int} tasks")]
async fn board_with_limit(world: &mut AppWorld, wip_limit: u32, current_count: u32) {
    world.board_state = Some(BoardState {
        wip_limit: Some(wip_limit),
        current_count,
    });
}

#[given(expr = "a task {string} below the horizon in an active project")]
async fn task_below_horizon_active(world: &mut AppWorld, name: String) {
    world.set_task_summary(&name, Placement::Below, ProjectStatus::Active);
}

#[given(expr = "a task {string} below the horizon in a pending project")]
async fn task_below_horizon_pending(world: &mut AppWorld, name: String) {
    world.set_task_summary(&name, Placement::Below, ProjectStatus::Pending);
}

#[when(expr = "a suggestion is requested")]
async fn suggestion_requested(world: &mut AppWorld) {
    let board = world
        .board_state
        .expect("scenario must set up a board state first");

    let candidates: Vec<_> = world.task_summaries.values().cloned().collect();

    let tangled_task_ids: std::collections::BTreeSet<_> = world
        .stored_tangles
        .iter()
        .flat_map(|t| t.task_ids.iter().copied())
        .collect();
    let graph = BlockingGraph {
        edges: world
            .relationships
            .iter()
            .map(|r| (r.from_task_id, r.to_task_id))
            .collect(),
        done_task_ids: std::collections::BTreeSet::new(),
        tangled_task_ids,
        // Every currently-stored tangle is still offerable in this
        // scenario suite: none of them has been "accepted onto the board"
        // (that distinction is exercised directly in `suggest`'s own unit
        // tests, not here).
        tangles: world.stored_tangles.clone(),
    };

    let settings = SuggestionSettings {
        cooldown_seconds: 3 * 24 * 3600,
        high_bounce_threshold: 3,
    };

    let now = Timestamp::from_unix_seconds(1_000).unwrap();
    let outcome = suggest(now, 42, &board, &candidates, &graph, &settings);
    world.last_outcome = Some(outcome);
}

#[then(expr = "the system offers nothing at all")]
async fn offers_nothing(world: &mut AppWorld) {
    assert_eq!(world.last_outcome, Some(Outcome::Full));
}

#[then(expr = "the system explains that no project is active")]
async fn explains_no_active_project(world: &mut AppWorld) {
    assert_eq!(
        world.last_outcome,
        Some(Outcome::Stuck(Blockage::NoActiveProject))
    );
}

#[then(expr = "the system explains that the backlog is empty")]
async fn explains_backlog_empty(world: &mut AppWorld) {
    assert_eq!(
        world.last_outcome,
        Some(Outcome::Stuck(Blockage::BacklogEmpty))
    );
}

fn offer(world: &AppWorld) -> &Offer {
    match world.last_outcome.as_ref() {
        Some(Outcome::Offer(offer)) => offer,
        other => panic!("expected an Offer, got {other:?}"),
    }
}

#[then(expr = "the system offers the tangle containing {string} and {string}")]
async fn offers_the_tangle(world: &mut AppWorld, a: String, b: String) {
    let a_id = world.core_task(&a);
    let b_id = world.core_task(&b);
    let found = offer(world).items.iter().any(|item| match item {
        OfferItem::Tangle(t) => t.task_ids.contains(&a_id) && t.task_ids.contains(&b_id),
        OfferItem::Task(_) => false,
    });
    assert!(
        found,
        "expected the offer to contain a tangle with {a:?} and {b:?}, got {:?}",
        offer(world).items
    );
}

#[then(expr = "neither {string} nor {string} is offered on its own")]
async fn neither_offered_individually(world: &mut AppWorld, a: String, b: String) {
    let a_id = world.core_task(&a);
    let b_id = world.core_task(&b);
    let has_individual = offer(world).items.iter().any(|item| match item {
        OfferItem::Task(t) => t.task_id == a_id || t.task_id == b_id,
        OfferItem::Tangle(_) => false,
    });
    assert!(
        !has_individual,
        "expected neither {a:?} nor {b:?} to be offered individually, got {:?}",
        offer(world).items
    );
}
