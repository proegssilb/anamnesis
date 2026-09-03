//! `tower::ServiceExt::oneshot` coverage for the runtime settings page
//! (`GET`/`POST /settings`): System-Admin-only on both directions, and
//! proof that editing a setting through the UI actually changes enforced
//! behaviour on the very next request — not just what a reload of the form
//! echoes back.

mod support;

use axum::body::Body;
use axum::http::{Response, StatusCode};

use support::{TestApp, body_text, set_active_project_limit, set_project_status};

#[tokio::test]
async fn get_settings_by_a_non_admin_is_forbidden() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let stranger_cookie = app.login_cookie_header("stranger", "stranger-token");

    let response = app.get("/settings", Some(&stranger_cookie)).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn get_settings_by_the_admin_shows_defaults() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");

    let response = app.get("/settings", Some(&admin_cookie)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(body.contains("Active project limit"));
    // `anamnesis_app::DEFAULT_ACTIVE_PROJECT_LIMIT` is 5, prefilled as the
    // input's value.
    assert!(body.contains(r#"value="5""#));
}

#[tokio::test]
async fn post_settings_by_a_non_admin_is_forbidden() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let stranger_cookie = app.login_cookie_header("stranger", "stranger-token");

    let response = app
        .post_form(
            "/settings",
            &[
                ("csrf_token", "stranger-token"),
                ("active_project_limit", "1"),
                ("cooldown_seconds", "0"),
                ("high_bounce_threshold", "3"),
                ("sweep_kind", "never"),
            ],
            Some(&stranger_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn post_settings_without_a_valid_csrf_token_is_rejected() {
    let app = TestApp::with_bootstrap_admin(false, "admin").await;
    let admin_cookie = app.login_cookie_header("admin", "admin-token");

    let response = app
        .post_form(
            "/settings",
            &[
                ("csrf_token", "wrong-token"),
                ("active_project_limit", "1"),
                ("cooldown_seconds", "0"),
                ("high_bounce_threshold", "3"),
                ("sweep_kind", "never"),
            ],
            Some(&admin_cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn updating_active_project_limit_through_settings_changes_the_enforced_limit() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    // Lower the active-project limit to 1 through the settings UI.
    set_active_project_limit(&app, 1, cookie).await;

    let area_path = support::new_area(&app, "Home", support::DEV_CSRF_TOKEN, cookie).await;
    let first_project =
        support::new_project_in(&app, &area_path, "One", support::DEV_CSRF_TOKEN, cookie).await;
    let second_project =
        support::new_project_in(&app, &area_path, "Two", support::DEV_CSRF_TOKEN, cookie).await;

    let first_active = set_project_status(
        &app,
        &first_project,
        "active",
        support::DEV_CSRF_TOKEN,
        cookie,
    )
    .await;
    assert_eq!(
        first_active.status(),
        StatusCode::SEE_OTHER,
        "the first project must fit under the new limit of 1"
    );

    let second_active = set_project_status(
        &app,
        &second_project,
        "active",
        support::DEV_CSRF_TOKEN,
        cookie,
    )
    .await;
    assert_eq!(
        second_active.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "a second Active project must now be refused -- proving the setting \
         edited through /settings is what transition_project_status enforces, \
         not the old hardcoded default of 5"
    );
    let body = body_text(second_active).await;
    assert!(body.contains("active project limit"));
}

#[tokio::test]
async fn updating_the_suggestion_cooldown_through_settings_changes_eligibility() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let (_, project_path) =
        support::new_active_project(&app, "Home", "Kitchen remodel", cookie).await;
    support::new_task(&app, &project_path, "Regrout the shower", cookie).await;

    // First view: the one backlog task is offered (stamping
    // `last_offered_at`) under the default ~3-day cooldown.
    let first = body_text(app.get("/board", cookie).await).await;
    assert!(
        first.contains("Regrout the shower"),
        "the only backlog task must be offered on the first view: {first}"
    );

    // Second view, moments later: it is the only backlog candidate and is
    // now on cooldown, so nothing is offered -- `Outcome::Stuck` explains
    // why (`docs/DOMAIN.md` §5's `AllOnCooldown`).
    let second = body_text(app.get("/board", cookie).await).await;
    assert!(
        !second.contains("Regrout the shower") || !second.contains("suggestion-prompt"),
        "on the default cooldown, the same lone candidate must not be \
         re-offered moments later: {second}"
    );

    // Now zero out the cooldown through the settings UI...
    let save = set_suggestion_cooldown(&app, 0, cookie).await;
    assert_eq!(save.status(), StatusCode::SEE_OTHER);

    // ...and the very same lone candidate is offered again immediately --
    // proving the cooldown edited through /settings is what the suggestion
    // engine actually used, not the old hardcoded default.
    let third = body_text(app.get("/board", cookie).await).await;
    assert!(
        third.contains("Regrout the shower") && third.contains("suggestion-prompt"),
        "with the cooldown set to 0, the lone candidate must be offered \
         again on the very next view: {third}"
    );
}

/// Posts the settings form with a chosen suggestion cooldown, leaving the
/// other settings at their bootstrap defaults. Local to this file (rather
/// than `support::set_active_project_limit`) because its callers vary the
/// opposite axis: that helper fixes the cooldown and varies the limit, this
/// test needs the limit fixed and the cooldown varied.
async fn set_suggestion_cooldown(
    app: &TestApp,
    cooldown_seconds: u32,
    cookie: Option<&str>,
) -> Response<Body> {
    app.post_form(
        "/settings",
        &[
            ("csrf_token", support::DEV_CSRF_TOKEN),
            ("active_project_limit", "5"),
            ("cooldown_seconds", &cooldown_seconds.to_string()),
            ("high_bounce_threshold", "3"),
            ("sweep_kind", "never"),
        ],
        cookie,
    )
    .await
}
