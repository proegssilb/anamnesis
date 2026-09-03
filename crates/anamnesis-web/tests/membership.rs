//! HTTP-level coverage for Gap 1: granting and revoking roles through the
//! UI (`crate::handlers::membership`), and — the security-critical half —
//! that the transport layer cannot be used to smuggle an escalation past
//! `anamnesis_app::use_cases::membership`'s own refusals.

mod support;

use axum::http::StatusCode;

use support::{
    DEV_CSRF_TOKEN, TestApp, body_text, location_of, new_area, new_area_with_project,
    new_project_in, set_active_project_limit,
};

async fn create_area(app: &TestApp, cookie: &str, csrf: &str, title: &str) -> String {
    new_area(app, title, csrf, Some(cookie)).await
}

async fn create_project(
    app: &TestApp,
    cookie: &str,
    csrf: &str,
    area_path: &str,
    title: &str,
) -> String {
    new_project_in(app, area_path, title, csrf, Some(cookie)).await
}

// --- Area members: grant lets a user in, revoke locks them back out ---

#[tokio::test]
async fn granting_an_area_role_through_the_ui_lets_that_user_view_it_and_revoking_removes_it() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = create_area(&app, &admin_cookie, "admin-token", "Home").await;

    let bob_cookie = app.login_cookie_header("bob", "bob-token");

    // Before any grant, Bob is refused.
    let before = app.get(&area_path, Some(&bob_cookie)).await;
    assert_eq!(before.status(), StatusCode::FORBIDDEN);

    // The admin grants Bob a Member role on the area.
    let grant = app
        .post_form(
            &format!("{area_path}/members"),
            &[
                ("csrf_token", "admin-token"),
                ("user_id", "bob"),
                ("role", "member"),
            ],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(
        grant.status(),
        StatusCode::SEE_OTHER,
        "grant must 303 redirect"
    );
    assert_eq!(location_of(&grant), area_path);

    // Bob can now view the area.
    let after_grant = app.get(&area_path, Some(&bob_cookie)).await;
    assert_eq!(after_grant.status(), StatusCode::OK);

    // The admin revokes it.
    let revoke = app
        .post_form(
            &format!("{area_path}/members/revoke"),
            &[("csrf_token", "admin-token"), ("user_id", "bob")],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(revoke.status(), StatusCode::SEE_OTHER);

    // Bob is refused again.
    let after_revoke = app.get(&area_path, Some(&bob_cookie)).await;
    assert_eq!(after_revoke.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn granting_an_area_role_without_a_valid_csrf_token_is_rejected() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = create_area(&app, &admin_cookie, "admin-token", "Home").await;

    let response = app
        .post_form(
            &format!("{area_path}/members"),
            &[
                ("csrf_token", "not-the-real-token"),
                ("user_id", "bob"),
                ("role", "member"),
            ],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let bob_cookie = app.login_cookie_header("bob", "bob-token");
    let bobs_view = app.get(&area_path, Some(&bob_cookie)).await;
    assert_eq!(
        bobs_view.status(),
        StatusCode::FORBIDDEN,
        "a rejected CSRF must not have granted anything"
    );
}

#[tokio::test]
async fn revoking_an_area_role_without_a_valid_csrf_token_is_rejected() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = create_area(&app, &admin_cookie, "admin-token", "Home").await;
    app.post_form(
        &format!("{area_path}/members"),
        &[
            ("csrf_token", "admin-token"),
            ("user_id", "bob"),
            ("role", "member"),
        ],
        Some(&admin_cookie),
    )
    .await;

    let response = app
        .post_form(
            &format!("{area_path}/members/revoke"),
            &[("csrf_token", "not-the-real-token"), ("user_id", "bob")],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let bob_cookie = app.login_cookie_header("bob", "bob-token");
    let bobs_view = app.get(&area_path, Some(&bob_cookie)).await;
    assert_eq!(
        bobs_view.status(),
        StatusCode::OK,
        "a rejected CSRF must not have revoked the earlier grant"
    );
}

// --- Project members: same round trip ---

#[tokio::test]
async fn granting_a_project_role_through_the_ui_lets_that_user_view_it_and_revoking_removes_it() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = create_area(&app, &admin_cookie, "admin-token", "Home").await;
    let project_path =
        create_project(&app, &admin_cookie, "admin-token", &area_path, "Repaint").await;

    let bob_cookie = app.login_cookie_header("bob", "bob-token");
    let before = app.get(&project_path, Some(&bob_cookie)).await;
    assert_eq!(before.status(), StatusCode::FORBIDDEN);

    let grant = app
        .post_form(
            &format!("{project_path}/members"),
            &[
                ("csrf_token", "admin-token"),
                ("user_id", "bob"),
                ("role", "member"),
            ],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(grant.status(), StatusCode::SEE_OTHER);

    let after_grant = app.get(&project_path, Some(&bob_cookie)).await;
    assert_eq!(after_grant.status(), StatusCode::OK);

    app.post_form(
        &format!("{project_path}/members/revoke"),
        &[("csrf_token", "admin-token"), ("user_id", "bob")],
        Some(&admin_cookie),
    )
    .await;

    let after_revoke = app.get(&project_path, Some(&bob_cookie)).await;
    assert_eq!(after_revoke.status(), StatusCode::FORBIDDEN);
}

// --- No privilege escalation, through the real HTTP surface ---

#[tokio::test]
async fn a_project_admin_of_one_area_cannot_grant_a_role_on_a_different_area_over_http() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let home_path = create_area(&app, &admin_cookie, "admin-token", "Home").await;
    let work_path = create_area(&app, &admin_cookie, "admin-token", "Work").await;

    // Priya administers Home only.
    let priya_cookie = app.login_cookie_header("priya", "priya-token");
    app.post_form(
        &format!("{home_path}/members"),
        &[
            ("csrf_token", "admin-token"),
            ("user_id", "priya"),
            ("role", "project_admin"),
        ],
        Some(&admin_cookie),
    )
    .await;
    // Confirm Priya really does administer Home (can grant there).
    let priya_can_grant_on_home = app
        .post_form(
            &format!("{home_path}/members"),
            &[
                ("csrf_token", "priya-token"),
                ("user_id", "carol"),
                ("role", "member"),
            ],
            Some(&priya_cookie),
        )
        .await;
    assert_eq!(priya_can_grant_on_home.status(), StatusCode::SEE_OTHER);

    // Priya tries to grant Bob a role on Work, which she does not administer.
    let escalation = app
        .post_form(
            &format!("{work_path}/members"),
            &[
                ("csrf_token", "priya-token"),
                ("user_id", "bob"),
                ("role", "member"),
            ],
            Some(&priya_cookie),
        )
        .await;
    assert_eq!(escalation.status(), StatusCode::FORBIDDEN);

    // Bob genuinely gained no access to Work.
    let bob_cookie = app.login_cookie_header("bob", "bob-token");
    let bobs_view = app.get(&work_path, Some(&bob_cookie)).await;
    assert_eq!(bobs_view.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_area_grant_form_cannot_be_used_to_smuggle_system_admin() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let area_path = create_area(&app, &admin_cookie, "admin-token", "Home").await;

    // A hand-crafted POST (bypassing the `<select>`, which never offers this
    // option in the first place) trying to write "system_admin" straight
    // into the area grant form.
    let response = app
        .post_form(
            &format!("{area_path}/members"),
            &[
                ("csrf_token", "admin-token"),
                ("user_id", "mallory"),
                ("role", "system_admin"),
            ],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "the role parser must refuse anything but member/project_admin"
    );

    // Mallory gained no System Admin capability -- `/users` still refuses her.
    let mallory_cookie = app.login_cookie_header("mallory", "mallory-token");
    let users_page = app.get("/users", Some(&mallory_cookie)).await;
    assert_eq!(users_page.status(), StatusCode::FORBIDDEN);
}

// --- /users: the one System-Admin-only place that can grant System Admin ---

#[tokio::test]
async fn the_users_page_is_forbidden_to_a_non_admin() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let bob_cookie = app.login_cookie_header("bob", "bob-token");

    let get = app.get("/users", Some(&bob_cookie)).await;
    assert_eq!(get.status(), StatusCode::FORBIDDEN);

    let post = app
        .post_form(
            "/users",
            &[("csrf_token", "bob-token"), ("user_id", "bob")],
            Some(&bob_cookie),
        )
        .await;
    assert_eq!(post.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn granting_and_revoking_system_admin_through_the_users_page() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");
    let priya_cookie = app.login_cookie_header("priya", "priya-token");

    // Priya cannot reach settings before being granted System Admin.
    let before = app.get("/settings", Some(&priya_cookie)).await;
    assert_eq!(before.status(), StatusCode::FORBIDDEN);

    let grant = app
        .post_form(
            "/users",
            &[("csrf_token", "admin-token"), ("user_id", "priya")],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(grant.status(), StatusCode::SEE_OTHER);
    assert_eq!(location_of(&grant), "/users");

    // Priya now has System-Admin-gated access.
    let after_grant = app.get("/settings", Some(&priya_cookie)).await;
    assert_eq!(after_grant.status(), StatusCode::OK);

    let users_page = body_text(app.get("/users", Some(&admin_cookie)).await).await;
    assert!(users_page.contains("priya"));

    // The admin revokes Priya (two admins remain -- "admin" itself, so this
    // is not the last-admin case).
    let revoke = app
        .post_form(
            "/users/revoke",
            &[("csrf_token", "admin-token"), ("user_id", "priya")],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(revoke.status(), StatusCode::SEE_OTHER);

    let after_revoke = app.get("/settings", Some(&priya_cookie)).await;
    assert_eq!(after_revoke.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn revoking_the_last_system_admin_through_the_ui_is_refused() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");

    let response = app
        .post_form(
            "/users/revoke",
            &[("csrf_token", "admin-token"), ("user_id", "admin")],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "refusing the last admin re-renders the page with an error, not a redirect"
    );
    let body = body_text(response).await;
    assert!(body.contains("last System Admin"));

    // The admin genuinely still has access.
    let still_admin = app.get("/settings", Some(&admin_cookie)).await;
    assert_eq!(still_admin.status(), StatusCode::OK);
}

#[tokio::test]
async fn granting_system_admin_without_a_valid_csrf_token_is_rejected() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");

    let response = app
        .post_form(
            "/users",
            &[("csrf_token", "not-the-real-token"), ("user_id", "priya")],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let priya_cookie = app.login_cookie_header("priya", "priya-token");
    let priyas_view = app.get("/settings", Some(&priya_cookie)).await;
    assert_eq!(priyas_view.status(), StatusCode::FORBIDDEN);
}

// --- Dev-bypass sanity: the dev-bypass user (already System Admin from
// bootstrap) can reach every membership surface with the fixed dev CSRF
// token -- proves the routes are wired end to end under the same mode
// `cargo run`'s dev bypass uses. ---

// --- Project status transitions: the area page's drag-and-drop hx branches
// (`crate::handlers::areas::transition_project_status_impl`,
// `render_area_lanes_fragment`) -- every plain `/status` post elsewhere in
// this test suite (here, and in `tests/settings.rs`, `tests/suggestion.rs`,
// `tests/tangle_board.rs`) is setup-only and asserts nothing about the
// response itself, so neither hx branch was covered anywhere. ---

#[tokio::test]
async fn transitioning_a_projects_status_via_hx_returns_all_three_area_lanes_as_oob_fragments() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;
    let (_, project_path) =
        new_area_with_project(&app, "Home", "Repaint", DEV_CSRF_TOKEN, cookie).await;

    let response = app
        .post_form_hx(
            &format!("{project_path}/status"),
            &[("csrf_token", DEV_CSRF_TOKEN), ("status", "active")],
            cookie,
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "an HX-Request status change returns the fragment directly, not a redirect"
    );
    let body = body_text(response).await;
    assert!(
        !body.contains("<!doctype html>") && !body.contains("<html"),
        "an HX-Request must get bare fragments, not the page shell: {body}"
    );
    assert!(body.contains("hx-swap-oob=\"true\""));
    assert!(body.contains(r#"id="pending-list""#));
    assert!(body.contains(r#"id="active-list""#));
    assert!(body.contains(r#"id="complete-list""#));
    assert!(
        body.contains("Repaint"),
        "the moved project must appear in the returned lanes: {body}"
    );
}

#[tokio::test]
async fn transitioning_a_projects_status_via_hx_silently_reverts_when_the_active_project_limit_is_exceeded()
 {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    set_active_project_limit(&app, 1, cookie).await;

    let (area_path, first_project) =
        new_area_with_project(&app, "Home", "One", DEV_CSRF_TOKEN, cookie).await;
    let second_project = new_project_in(&app, &area_path, "Two", DEV_CSRF_TOKEN, cookie).await;

    // Fill the limit of 1 with the first project.
    let first_active = app
        .post_form(
            &format!("{first_project}/status"),
            &[("csrf_token", DEV_CSRF_TOKEN), ("status", "active")],
            cookie,
        )
        .await;
    assert_eq!(first_active.status(), StatusCode::SEE_OTHER);

    // The second project's hx status change silently reverts -- 200 with
    // the unchanged lanes, no error text, exactly like
    // `crate::handlers::projects::raise_project_task_impl`'s
    // `WipLimitExceeded` branch on the same shape of problem.
    let second_active = app
        .post_form_hx(
            &format!("{second_project}/status"),
            &[("csrf_token", DEV_CSRF_TOKEN), ("status", "active")],
            cookie,
        )
        .await;
    assert_eq!(second_active.status(), StatusCode::OK);
    let body = body_text(second_active).await;
    assert!(
        !body.contains("active project limit"),
        "the hx path has no per-card spot to show the limit error: {body}"
    );

    // The true (unchanged) DB state: "Two" is still Pending, not Active.
    let area_page = body_text(app.get(&area_path, cookie).await).await;
    let pending_section = area_page
        .split(r#"id="pending-list""#)
        .nth(1)
        .and_then(|s| s.split(r#"id="active-list""#).next())
        .expect("area page has a pending-list section before active-list");
    assert!(
        pending_section.contains("Two"),
        "a reverted hx status change must leave the project in its original lane: {area_page}"
    );
}

#[tokio::test]
async fn dev_bypass_admin_can_grant_an_area_role() {
    let app = TestApp::new(true).await;
    let area_path = location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", DEV_CSRF_TOKEN),
                ("title", "Home"),
                ("description", ""),
            ],
            None,
        )
        .await,
    )
    .to_string();

    let grant = app
        .post_form(
            &format!("{area_path}/members"),
            &[
                ("csrf_token", DEV_CSRF_TOKEN),
                ("user_id", "bob"),
                ("role", "member"),
            ],
            None,
        )
        .await;
    assert_eq!(grant.status(), StatusCode::SEE_OTHER);
}
