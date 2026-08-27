//! `tower::ServiceExt::oneshot` coverage for placeable tangles
//! (`docs/DOMAIN.md`'s Tangle section): a knotted pair is offered as a
//! tangle from the suggestion prompt, accepting the offer places it on the
//! board as its own card, and it can be dropped back below the horizon.

mod support;

use axum::http::StatusCode;

use anamnesis_app::TangleRepository;
use support::{TestApp, body_text, location_of};

#[tokio::test]
async fn accepting_a_tangle_offer_places_it_and_it_can_be_dropped_back() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let area_path = location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Home"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    let project_path = location_of(
        &app.post_form(
            &format!("{area_path}/projects"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Kitchen remodel"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    app.post_form(
        &format!("{project_path}/status"),
        &[
            ("csrf_token", support::DEV_CSRF_TOKEN),
            ("status", "active"),
        ],
        cookie,
    )
    .await;

    let task_a_path = location_of(
        &app.post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Design the layout"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    let task_b_path = location_of(
        &app.post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Order the tile"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    let task_b_id = task_b_path.trim_start_matches("/tasks/").to_string();
    let task_a_id = task_a_path.trim_start_matches("/tasks/").to_string();

    // A blocks B, and B blocks A: a knotted pair.
    let block_ab = app
        .post_form(
            &format!("{task_a_path}/relationships"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("to_task_id", &task_b_id),
                ("kind", "blocks"),
            ],
            cookie,
        )
        .await;
    assert_eq!(block_ab.status(), StatusCode::SEE_OTHER);
    let block_ba = app
        .post_form(
            &format!("{task_b_path}/relationships"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("to_task_id", &task_a_id),
                ("kind", "blocks"),
            ],
            cookie,
        )
        .await;
    assert_eq!(block_ba.status(), StatusCode::SEE_OTHER);

    // Viewing the board runs detection and offers the tangle in place of
    // its (individually ineligible) knotted tasks.
    let board_body = body_text(app.get("/board", cookie).await).await;
    assert!(
        board_body.contains("knotted together"),
        "the board must offer the tangle from the suggestion prompt: {board_body}"
    );

    let tangle = {
        let active = app.store.list_active().await.unwrap();
        assert_eq!(
            active.len(),
            1,
            "exactly one tangle must have been detected"
        );
        active[0].clone()
    };
    assert_eq!(tangle.task_ids.len(), 2);
    assert!(
        !tangle.frozen,
        "an offered tangle is still below the horizon"
    );

    // Accept the offer: it must be placed onto the board.
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

    // The board now renders it as a card, not just a suggestion.
    let after_accept = body_text(app.get("/board", cookie).await).await;
    assert!(
        after_accept.contains("tangle-card"),
        "the placed tangle must render as a card on the board: {after_accept}"
    );
    assert!(
        after_accept.contains("Drop back"),
        "a placed, unresolved tangle card offers a way to drop it back: {after_accept}"
    );

    // Drop it back below the horizon.
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
