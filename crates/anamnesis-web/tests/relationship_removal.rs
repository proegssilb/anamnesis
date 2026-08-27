//! `tower::ServiceExt::oneshot` coverage for removing a relationship edge
//! through the UI (`docs/DOMAIN.md` §3) — the fix for the functional dead
//! end where a relationship, once created, could never be taken back off:
//! `POST /tasks/{id}/relationships/{relationship_id}/delete`.

mod support;

use axum::http::StatusCode;

use anamnesis_app::{BoardQuery, RelationshipRepository, TangleRepository};
use support::{TestApp, body_text, location_of};

/// Creates two tasks under a fresh, active project, links them with a
/// `relates_to` edge from `a` to `b`, and returns `(task_a_path, task_b_path,
/// relationship_id)`.
async fn linked_pair(app: &TestApp) -> (String, String, uuid::Uuid) {
    let cookie: Option<&str> = None; // dev-auth-bypass needs no cookie at all.

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

    let create = app
        .post_form(
            &format!("{task_a_path}/relationships"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("to_task_id", &task_b_id),
                ("kind", "relates_to"),
            ],
            cookie,
        )
        .await;
    assert_eq!(create.status(), StatusCode::SEE_OTHER);

    let task_a_id = task_a_path.trim_start_matches("/tasks/").parse().unwrap();
    let all = app
        .store
        .list_for_task(anamnesis_core::TaskId::new(task_a_id))
        .await
        .unwrap();
    assert_eq!(all.len(), 1, "exactly one relationship must exist");
    let relationship_id = all[0].id.to_string().parse().unwrap();

    (task_a_path, task_b_path, relationship_id)
}

#[tokio::test]
async fn deleting_a_relationship_removes_it_from_the_task_page() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (task_a_path, task_b_path, relationship_id) = linked_pair(&app).await;

    let before = body_text(app.get(&task_a_path, cookie).await).await;
    assert!(
        before.contains("Order the tile"),
        "the relationship must render before deletion: {before}"
    );

    let delete = app
        .post_form(
            &format!("{task_a_path}/relationships/{relationship_id}/delete"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(delete.status(), StatusCode::SEE_OTHER);
    assert_eq!(location_of(&delete), task_a_path);

    let after_a = body_text(app.get(&task_a_path, cookie).await).await;
    assert!(
        !after_a.contains("Order the tile"),
        "the relationship must be gone from task A's page: {after_a}"
    );
    let after_b = body_text(app.get(&task_b_path, cookie).await).await;
    assert!(
        !after_b.contains("Design the layout"),
        "the relationship must also be gone from task B's (reverse) page: {after_b}"
    );

    let remaining = anamnesis_app::RelationshipRepository::load(
        app.store.as_ref(),
        anamnesis_core::RelationshipId::new(relationship_id),
    )
    .await
    .unwrap();
    assert!(remaining.is_none(), "the edge itself must be gone");
}

#[tokio::test]
async fn deleting_a_relationship_from_the_reverse_side_also_works() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (_task_a_path, task_b_path, relationship_id) = linked_pair(&app).await;

    // B is the *to* side of the edge, not the *from* side -- deleting from
    // B's own task page must still work.
    let delete = app
        .post_form(
            &format!("{task_b_path}/relationships/{relationship_id}/delete"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(delete.status(), StatusCode::SEE_OTHER);
    assert_eq!(location_of(&delete), task_b_path);

    let remaining = anamnesis_app::RelationshipRepository::load(
        app.store.as_ref(),
        anamnesis_core::RelationshipId::new(relationship_id),
    )
    .await
    .unwrap();
    assert!(remaining.is_none(), "the edge must be gone either way");
}

#[tokio::test]
async fn deleting_a_relationship_without_a_valid_csrf_token_is_rejected() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let cookie = app.login_cookie_header("admin", "the-real-token");
    let (task_a_path, _task_b_path, relationship_id) =
        linked_pair_authed(&app, &cookie, "the-real-token").await;

    let delete = app
        .post_form(
            &format!("{task_a_path}/relationships/{relationship_id}/delete"),
            &[("csrf_token", "not-the-right-token")],
            Some(&cookie),
        )
        .await;
    assert_eq!(delete.status(), StatusCode::FORBIDDEN);

    let remaining = anamnesis_app::RelationshipRepository::load(
        app.store.as_ref(),
        anamnesis_core::RelationshipId::new(relationship_id),
    )
    .await
    .unwrap();
    assert!(
        remaining.is_some(),
        "a bad CSRF token must not delete the edge"
    );
}

#[tokio::test]
async fn a_user_without_permission_cannot_delete_a_relationship() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let (task_a_path, _task_b_path, relationship_id) =
        linked_pair_authed(&app, &admin_cookie, "admin-token").await;

    let stranger_cookie = app.login_cookie_header("stranger", "stranger-token");
    let delete = app
        .post_form(
            &format!("{task_a_path}/relationships/{relationship_id}/delete"),
            &[("csrf_token", "stranger-token")],
            Some(&stranger_cookie),
        )
        .await;
    assert_eq!(delete.status(), StatusCode::FORBIDDEN);

    let remaining = anamnesis_app::RelationshipRepository::load(
        app.store.as_ref(),
        anamnesis_core::RelationshipId::new(relationship_id),
    )
    .await
    .unwrap();
    assert!(
        remaining.is_some(),
        "a stranger with no grant must not be able to delete the edge"
    );
}

/// As [`linked_pair`], but authenticated as `cookie`'s user (dev-auth-bypass
/// off) rather than relying on the dev-bypass identity.
async fn linked_pair_authed(
    app: &TestApp,
    cookie: &str,
    csrf_token: &str,
) -> (String, String, uuid::Uuid) {
    let cookie = Some(cookie);

    let area_path = location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", csrf_token),
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
                ("csrf_token", csrf_token),
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
        &[("csrf_token", csrf_token), ("status", "active")],
        cookie,
    )
    .await;

    let task_a_path = location_of(
        &app.post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", csrf_token),
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
                ("csrf_token", csrf_token),
                ("title", "Order the tile"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    let task_b_id = task_b_path.trim_start_matches("/tasks/").to_string();

    let create = app
        .post_form(
            &format!("{task_a_path}/relationships"),
            &[
                ("csrf_token", csrf_token),
                ("to_task_id", &task_b_id),
                ("kind", "relates_to"),
            ],
            cookie,
        )
        .await;
    assert_eq!(create.status(), StatusCode::SEE_OTHER);

    let task_a_id = task_a_path.trim_start_matches("/tasks/").parse().unwrap();
    let all = app
        .store
        .list_for_task(anamnesis_core::TaskId::new(task_a_id))
        .await
        .unwrap();
    assert_eq!(all.len(), 1, "exactly one relationship must exist");
    let relationship_id = all[0].id.to_string().parse().unwrap();

    (task_a_path, task_b_path, relationship_id)
}

/// The full untangle loop, end to end: two tasks block each other, a tangle
/// is detected and accepted onto the board, the blocking relationship is
/// removed *through the HTTP route this change adds*, and the tangle then
/// resolves and lands in the Done column. This is the whole point of the
/// change -- previously there was no way to close this loop at all.
#[tokio::test]
async fn removing_a_blocking_edge_through_the_route_resolves_the_tangle_into_done() {
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
    let task_a_id = task_a_path.trim_start_matches("/tasks/").to_string();
    let task_b_id = task_b_path.trim_start_matches("/tasks/").to_string();

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

    // Viewing the board runs detection and offers the tangle.
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

    // Accept the offer onto the board.
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

    let placed = anamnesis_app::TangleRepository::load(app.store.as_ref(), tangle.id)
        .await
        .unwrap()
        .expect("the tangle must still exist after being placed");
    assert!(placed.placement.is_on_board(), "accepting must place it");
    assert!(placed.frozen, "placing must freeze its membership");

    // Untangle it: remove the A-blocks-B edge *through the HTTP route this
    // change adds* -- the one edge that, with its mirror still standing,
    // leaves no cycle in the live blocking graph.
    let a_task_id = anamnesis_core::TaskId::new(task_a_id.parse().unwrap());
    let a_relationships = app.store.list_for_task(a_task_id).await.unwrap();
    let blocking_edge = a_relationships
        .iter()
        .find(|r| r.kind_id == anamnesis_core::builtin_blocks().id && r.from_task_id == a_task_id)
        .expect("the A-blocks-B edge must exist");

    let remove = app
        .post_form(
            &format!("{task_a_path}/relationships/{}/delete", blocking_edge.id),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(remove.status(), StatusCode::SEE_OTHER);

    // Viewing the board again must resolve the now-acyclic frozen tangle
    // and move it into the Done column.
    let after_body = body_text(app.get("/board", cookie).await).await;

    let resolved = anamnesis_app::TangleRepository::load(app.store.as_ref(), tangle.id)
        .await
        .unwrap()
        .expect("the tangle row still exists once resolved");
    assert!(
        resolved.resolved_at.is_some(),
        "removing the edge must resolve the tangle: {after_body}"
    );

    let columns = app.store.columns_with_items().await.unwrap();
    let done_column = columns
        .iter()
        .find(|c| c.column.is_done)
        .expect("the board has a Done column");
    match resolved.placement {
        anamnesis_core::Placement::OnBoard { column, .. } => {
            assert_eq!(
                column, done_column.column.id,
                "a resolved tangle must land in the Done column"
            );
        }
        anamnesis_core::Placement::Below => {
            panic!("a resolved, previously-placed tangle must stay on the board")
        }
    }
}
