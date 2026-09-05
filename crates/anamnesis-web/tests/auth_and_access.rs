//! `tower::ServiceExt::oneshot` coverage for authentication and access
//! control: unauthenticated requests redirect to `/login`, a mutating POST
//! without a valid CSRF token is rejected, and a user with no grant on an
//! Area cannot view it (`docs/DOMAIN.md` §3).

mod support;

use axum::http::StatusCode;

use anamnesis_core::UserId;
use support::{TestApp, location_of};

#[tokio::test]
async fn unauthenticated_get_redirects_to_login() {
    let app = TestApp::new(false).await;
    let response = app.get("/areas", None).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location_of(&response), "/login");
}

#[tokio::test]
async fn unauthenticated_board_view_also_redirects_to_login() {
    let app = TestApp::new(false).await;
    let response = app.get("/board", None).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location_of(&response), "/login");
}

#[tokio::test]
async fn post_without_a_valid_csrf_token_is_rejected() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let cookie = app.login_cookie_header("admin", "the-real-token");

    let response = app
        .post_form(
            "/areas",
            &[
                ("csrf_token", "not-the-right-token"),
                ("title", "Home"),
                ("description", ""),
            ],
            Some(&cookie),
        )
        .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn post_with_no_csrf_field_at_all_is_rejected() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let cookie = app.login_cookie_header("admin", "the-real-token");

    // A malformed form (missing the required `csrf_token` field entirely)
    // fails to deserialize before the handler's own CSRF check ever runs --
    // still correctly rejected, just via a 422 from the `Form` extractor
    // rather than the handler's 403.
    let response = app
        .post_form("/areas", &[("title", "Home")], Some(&cookie))
        .await;

    assert!(
        response.status() == StatusCode::UNPROCESSABLE_ENTITY
            || response.status() == StatusCode::BAD_REQUEST,
        "expected a client-error rejection, got {}",
        response.status()
    );
}

#[tokio::test]
async fn a_user_with_no_grant_cannot_view_an_area() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");

    let create = app
        .post_form(
            "/areas",
            &[
                ("csrf_token", "admin-token"),
                ("title", "Home"),
                ("description", ""),
            ],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(create.status(), StatusCode::SEE_OTHER);
    let area_path = location_of(&create).to_string();

    // The admin who created it can view it.
    let admin_view = app.get(&area_path, Some(&admin_cookie)).await;
    assert_eq!(admin_view.status(), StatusCode::OK);

    // A different, ungranted user cannot.
    let stranger_cookie = app.login_cookie_header("stranger", "stranger-token");
    let stranger_view = app.get(&area_path, Some(&stranger_cookie)).await;
    assert_eq!(stranger_view.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_area_grid_is_empty_for_a_user_with_no_grants_rather_than_forbidden() {
    // docs/DOMAIN.md gives `list_areas` no per-area scope of its own to
    // check (see crate::handlers::areas's module doc comment) -- resolved
    // as: any authenticated user may load the grid, filtered down to the
    // areas they actually hold a role on. A fully ungranted user therefore
    // sees an empty grid, not a 403.
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    app.post_form(
        "/areas",
        &[
            ("csrf_token", "admin-token"),
            ("title", "Home"),
            ("description", ""),
        ],
        Some(&admin_cookie),
    )
    .await;

    let stranger_cookie = app.login_cookie_header("stranger", "stranger-token");
    let response = app.get("/areas", Some(&stranger_cookie)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = support::body_text(response).await;
    assert!(
        !body.contains("Home"),
        "an ungranted user must not see an area they hold no role on"
    );
}

/// Records the groups a login would have recorded, without one — the
/// `user_groups` write `crate::handlers::login` performs after the identity
/// provider hands back its claims. Tests reach the port directly because
/// there is no OIDC provider behind a `TestApp`.
async fn record_groups(app: &TestApp, user: &str, groups: &[&str]) {
    let groups: Vec<String> = groups.iter().map(|g| (*g).to_string()).collect();
    app.state
        .group_membership_write
        .replace_user_groups(&UserId::new(user), &groups)
        .await
        .expect("recording the identity provider's groups");
}

#[tokio::test]
async fn a_mapped_admin_group_grants_system_admin_and_unmapping_it_denies_at_once() {
    // The behaviour that motivated storing groups in the database rather
    // than in the session cookie: a mapping is joined at request time, so
    // both granting and revoking it take effect for an already-signed-in
    // user with no new login.
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    record_groups(&app, "grouped", &["anamnesis-admins"]).await;
    let grouped_cookie = app.login_cookie_header("grouped", "grouped-token");

    // A `user_groups` row on its own grants nothing.
    assert_eq!(
        app.get("/users", Some(&grouped_cookie)).await.status(),
        StatusCode::FORBIDDEN
    );

    let granted = app
        .post_form(
            "/users/groups",
            &[("csrf_token", "admin-token"), ("group", "anamnesis-admins")],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(granted.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        app.get("/users", Some(&grouped_cookie)).await.status(),
        StatusCode::OK,
        "the mapping must apply without a fresh login"
    );

    let revoked = app
        .post_form(
            "/users/groups/revoke",
            &[("csrf_token", "admin-token"), ("group", "anamnesis-admins")],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(revoked.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        app.get("/users", Some(&grouped_cookie)).await.status(),
        StatusCode::FORBIDDEN,
        "revoking the mapping must deny immediately, not at next login"
    );
}

#[tokio::test]
async fn a_group_role_on_an_area_lets_its_members_view_that_area_only() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = support::new_area(&app, "Home", "admin-token", Some(&admin_cookie)).await;
    let other_path = support::new_area(&app, "Work", "admin-token", Some(&admin_cookie)).await;

    record_groups(&app, "grouped", &["mealie-admins"]).await;
    let grouped_cookie = app.login_cookie_header("grouped", "grouped-token");
    assert_eq!(
        app.get(&area_path, Some(&grouped_cookie)).await.status(),
        StatusCode::FORBIDDEN
    );

    let granted = app
        .post_form(
            &format!("{area_path}/member-groups"),
            &[
                ("csrf_token", "admin-token"),
                ("group", "mealie-admins"),
                ("role", "member"),
            ],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(granted.status(), StatusCode::SEE_OTHER);

    assert_eq!(
        app.get(&area_path, Some(&grouped_cookie)).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        app.get(&other_path, Some(&grouped_cookie)).await.status(),
        StatusCode::FORBIDDEN,
        "the grant is scoped to the one area it was made on"
    );
}

#[tokio::test]
async fn a_group_cannot_be_granted_system_admin_through_an_area_form() {
    // `parse_grantable_role` refuses `system_admin` at the transport edge,
    // before `grant_area_group_role`'s own refusal ever runs — the same
    // belt-and-suspenders the per-user area form gets.
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = support::new_area(&app, "Home", "admin-token", Some(&admin_cookie)).await;

    let response = app
        .post_form(
            &format!("{area_path}/member-groups"),
            &[
                ("csrf_token", "admin-token"),
                ("group", "anamnesis-admins"),
                ("role", "system_admin"),
            ],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
