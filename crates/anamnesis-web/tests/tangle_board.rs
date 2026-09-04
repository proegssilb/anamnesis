//! `tower::ServiceExt::oneshot` coverage for placeable tangles
//! (`docs/DOMAIN.md`'s Tangle section): a knotted pair is offered as a
//! tangle from the suggestion prompt, accepting the offer places it on the
//! board as its own card, and it can be dropped back below the horizon.

mod support;

use axum::http::StatusCode;

use anamnesis_app::TangleRepository;
use anamnesis_core::Tangle;
use support::{TestApp, body_text};

/// Creates an active project with two tasks that mutually block each other
/// -- a knotted pair -- and returns their task paths.
async fn setup_knotted_pair(app: &TestApp, cookie: Option<&str>) -> (String, String) {
    let (_, project_path) =
        support::new_active_project(app, "Home", "Kitchen remodel", cookie).await;
    let task_a_path = support::new_task(app, &project_path, "Design the layout", cookie).await;
    let task_b_path = support::new_task(app, &project_path, "Order the tile", cookie).await;
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

/// Runs a detection pass, then asserts the knotted pair is offered from the
/// board's suggestion prompt in place of its individually-ineligible tasks,
/// and returns the detected tangle.
///
/// The pass is explicit because the board GET no longer runs one: detection
/// moved onto `anamnesis_web::tangles`'s scheduled ticker, so the board reads
/// tangle state rather than computing it. This is the same call that ticker
/// makes.
async fn assert_board_offers_the_tangle(app: &TestApp, cookie: Option<&str>) -> Tangle {
    app.refresh_tangles().await;
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

/// The behaviour change moving detection off the read path makes, pinned: a
/// board GET is a pure *read* of tangle state and no longer computes it.
///
/// The knot exists in the blocking graph the whole time; what differs is only
/// whether anything has looked. If detection ever creeps back into a handler,
/// this is what fails.
#[tokio::test]
async fn viewing_the_board_does_not_run_tangle_detection() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    setup_knotted_pair(&app, cookie).await;

    let board_body = body_text(app.get("/board", cookie).await).await;
    assert!(
        app.store.list_active().await.unwrap().is_empty(),
        "a board GET must not detect tangles -- that is the scheduled \
         ticker's job: {board_body}"
    );

    app.refresh_tangles().await;
    assert_eq!(
        app.store.list_active().await.unwrap().len(),
        1,
        "the very same knot must be detected once a scheduled pass runs"
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
