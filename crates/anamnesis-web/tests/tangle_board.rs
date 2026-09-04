//! `tower::ServiceExt::oneshot` coverage for placeable tangles
//! (`docs/DOMAIN.md`'s Tangle section): a knotted pair is offered as a
//! tangle from the suggestion prompt, accepting the offer places it on the
//! board as its own card, and it can be dropped back below the horizon.

mod support;

use axum::http::StatusCode;

use anamnesis_app::TangleRepository;
use anamnesis_core::{Tangle, TaskId};
use support::{TestApp, body_text};

/// Creates an active project with two tasks and returns their task paths --
/// not yet knotted.
async fn setup_pair(app: &TestApp, cookie: Option<&str>) -> (String, String) {
    let (_, project_path) =
        support::new_active_project(app, "Home", "Kitchen remodel", cookie).await;
    let task_a_path = support::new_task(app, &project_path, "Design the layout", cookie).await;
    let task_b_path = support::new_task(app, &project_path, "Order the tile", cookie).await;
    (task_a_path, task_b_path)
}

/// Creates an active project with two tasks that mutually block each other
/// -- a knotted pair -- and returns their task paths.
///
/// The knot is built through the real relationship route, so detection has
/// already run by the time this returns: `anamnesis_web::tangles` is driven
/// by exactly this event.
async fn setup_knotted_pair(app: &TestApp, cookie: Option<&str>) -> (String, String) {
    let (task_a_path, task_b_path) = setup_pair(app, cookie).await;
    support::knot_together(
        app,
        &task_a_path,
        &task_b_path,
        support::DEV_CSRF_TOKEN,
        cookie,
    )
    .await;
    (task_a_path, task_b_path)
}

/// Asserts the knotted pair is offered from the board's suggestion prompt in
/// place of its individually-ineligible tasks, and returns the detected
/// tangle.
///
/// No explicit detection pass: the board GET does not run one (that moved off
/// the read path), and does not need to. Creating the `blocks` edges through
/// the relationship route already ran it.
async fn assert_board_offers_the_tangle(app: &TestApp, cookie: Option<&str>) -> Tangle {
    let board_body = body_text(app.get("/board", cookie).await).await;
    assert!(
        board_body.contains("knotted together"),
        "the board must offer the tangle from the suggestion prompt: {board_body}"
    );

    let active = app.store.list_active().await.unwrap();
    assert_eq!(
        active.len(),
        1,
        "exactly one tangle must have been detected"
    );
    let tangle = active[0].clone();
    assert_eq!(tangle.task_ids.len(), 2);
    assert!(
        !tangle.frozen,
        "an offered tangle is still below the horizon"
    );
    tangle
}

/// Accepts the tangle's offer and asserts it is now placed and frozen, both
/// in storage and as a rendered board card offering a way to drop it back.
async fn accept_and_assert_placed(app: &TestApp, tangle: &Tangle, cookie: Option<&str>) {
    let accept = app
        .post_form(
            "/board/suggestion/accept-tangle",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("tangle_id", &tangle.id.to_string()),
            ],
            cookie,
        )
        .await;
    assert_eq!(accept.status(), StatusCode::SEE_OTHER);

    let placed = app
        .store
        .load(tangle.id)
        .await
        .unwrap()
        .expect("the tangle must still exist after being placed");
    assert!(placed.placement.is_on_board(), "accepting must place it");
    assert!(placed.frozen, "placing must freeze its membership");

    let after_accept = body_text(app.get("/board", cookie).await).await;
    assert!(
        after_accept.contains("tangle-card"),
        "the placed tangle must render as a card on the board: {after_accept}"
    );
    assert!(
        after_accept.contains("Drop back"),
        "a placed, unresolved tangle card offers a way to drop it back: {after_accept}"
    );
}

/// Drops the tangle back below the horizon and asserts it is no longer
/// placed or frozen, and no longer rendered as a board card.
async fn drop_and_assert_below_horizon(app: &TestApp, tangle: &Tangle, cookie: Option<&str>) {
    let drop = app
        .post_form(
            &format!("/tangles/{}/drop", tangle.id),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(drop.status(), StatusCode::SEE_OTHER);

    let dropped = app
        .store
        .load(tangle.id)
        .await
        .unwrap()
        .expect("the tangle still exists after being dropped");
    assert!(!dropped.placement.is_on_board(), "drop must send it below");
    assert!(!dropped.frozen, "dropping must unfreeze it");

    let after_drop = body_text(app.get("/board", cookie).await).await;
    assert!(
        !after_drop.contains("tangle-card"),
        "a dropped tangle must no longer render as a board card: {after_drop}"
    );
}

fn task_id_of(task_path: &str) -> TaskId {
    TaskId::new(task_path.trim_start_matches("/tasks/").parse().unwrap())
}

/// Writes one `blocks` edge straight through the store, deliberately
/// bypassing the relationship route -- which is now the thing that triggers
/// detection.
async fn insert_blocks_edge_behind_the_routes(app: &TestApp, from: TaskId, to: TaskId) {
    async fn project_of(app: &TestApp, task: TaskId) -> anamnesis_core::ProjectId {
        anamnesis_app::TaskRepository::load(app.store.as_ref(), task)
            .await
            .unwrap()
            .expect("the task exists")
            .task
            .project_id
    }

    let edge = anamnesis_core::create_relationship(
        anamnesis_core::RelationshipId::new(app.state.id_gen.next()),
        from,
        project_of(app, from).await,
        to,
        project_of(app, to).await,
        &anamnesis_core::builtin_blocks(),
        app.state.clock.now(),
    )
    .expect("a blocks edge between two distinct tasks is valid");
    anamnesis_app::RelationshipRepository::insert(app.store.as_ref(), &edge)
        .await
        .unwrap();
}

/// The behaviour change moving detection off the read path makes, pinned: a
/// board GET is a pure *read* of tangle state and no longer computes it.
///
/// The knot has to be built behind the relationship route to show this, since
/// going through the route would detect it on the way in. That is not a
/// contrivance to make the test pass -- it is the point. The knot exists in
/// the blocking graph the whole time; what differs is only whether anything
/// has looked. If detection ever creeps back into a handler, this is what
/// fails.
#[tokio::test]
async fn viewing_the_board_does_not_run_tangle_detection() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let (task_a_path, task_b_path) = setup_pair(&app, cookie).await;
    let (task_a, task_b) = (task_id_of(&task_a_path), task_id_of(&task_b_path));
    insert_blocks_edge_behind_the_routes(&app, task_a, task_b).await;
    insert_blocks_edge_behind_the_routes(&app, task_b, task_a).await;

    let board_body = body_text(app.get("/board", cookie).await).await;
    assert!(
        app.store.list_active().await.unwrap().is_empty(),
        "a board GET must not detect tangles -- only a mutation or the \
         backstop does that: {board_body}"
    );

    app.refresh_tangles().await;
    assert_eq!(
        app.store.list_active().await.unwrap().len(),
        1,
        "the very same knot must be detected once a pass runs"
    );
}

/// The event path, pinned from the other side: closing the knot through the
/// relationship route detects it there and then -- no board GET, no explicit
/// pass, nothing waiting on a timer.
///
/// This is the immediate consistency that moving detection off the read path
/// gave up and event-driving it gets back, so it is worth asserting without
/// any HTTP read in between to blur where the work happened.
#[tokio::test]
async fn creating_the_blocking_edge_detects_the_tangle_immediately() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let (task_a_path, task_b_path) = setup_pair(&app, cookie).await;

    // Half a knot: A blocks B is not a cycle, so the first edit must detect
    // nothing. Pinning this separately keeps the test honest about detecting
    // a *tangle* rather than merely reacting to a write.
    support::create_blocking_edge(
        &app,
        &task_a_path,
        &task_b_path,
        support::DEV_CSRF_TOKEN,
        cookie,
    )
    .await;
    assert!(
        app.store.list_active().await.unwrap().is_empty(),
        "one blocking edge is not a cycle and must not be a tangle"
    );

    support::create_blocking_edge(
        &app,
        &task_b_path,
        &task_a_path,
        support::DEV_CSRF_TOKEN,
        cookie,
    )
    .await;
    let active = app.store.list_active().await.unwrap();
    assert_eq!(
        active.len(),
        1,
        "closing the cycle through the relationship route must detect the \
         tangle in that same request"
    );
    assert_eq!(active[0].task_ids.len(), 2);
}

/// The delete half of the event path: removing a `blocks` edge re-derives the
/// tangle set in that same request too.
///
/// `relationship_removal.rs` covers what removal does to a *placed* tangle
/// (`resolve_frozen_tangles` closes it into Done). This covers the simpler
/// unfrozen case, and covers it specifically as an event: detection has to
/// run on the delete route, not only the create one.
#[tokio::test]
async fn deleting_the_blocking_edge_resolves_the_tangle_immediately() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let (task_a_path, _) = setup_knotted_pair(&app, cookie).await;
    assert_eq!(app.store.list_active().await.unwrap().len(), 1);

    let task_a = task_id_of(&task_a_path);
    let edge = anamnesis_app::RelationshipRepository::list_for_task(app.store.as_ref(), task_a)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.kind_id == anamnesis_core::builtin_blocks().id && r.from_task_id == task_a)
        .expect("the A-blocks-B edge must exist");
    let removed = app
        .post_form(
            &format!("{task_a_path}/relationships/{}/delete", edge.id),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(removed.status(), StatusCode::SEE_OTHER);

    assert!(
        app.store.list_active().await.unwrap().is_empty(),
        "breaking the cycle through the delete route must resolve the tangle \
         in that same request"
    );
}

#[tokio::test]
async fn accepting_a_tangle_offer_places_it_and_it_can_be_dropped_back() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    setup_knotted_pair(&app, cookie).await;
    let tangle = assert_board_offers_the_tangle(&app, cookie).await;
    accept_and_assert_placed(&app, &tangle, cookie).await;
    drop_and_assert_below_horizon(&app, &tangle, cookie).await;
}
