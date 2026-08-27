//! `tower::ServiceExt::oneshot` coverage for authentication and access
//! control: unauthenticated requests redirect to `/login`, a mutating POST
//! without a valid CSRF token is rejected, and a user with no grant on an
//! Area cannot view it (`docs/DOMAIN.md` §3).

mod support;

use axum::http::StatusCode;

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
