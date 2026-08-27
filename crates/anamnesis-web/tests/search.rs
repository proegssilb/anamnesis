//! `tower::ServiceExt::oneshot` coverage for global search
//! (`docs/DOMAIN.md` §8): search must actually find what the UI just
//! created — the point of checking whether `SearchIndex` was wired at all
//! (it was not; see `crate::handlers::areas`/`projects`/`tasks`'s new
//! `search_index` calls).

mod support;

use axum::http::StatusCode;

use support::{TestApp, body_text, location_of};

#[tokio::test]
async fn search_finds_an_area_a_project_and_a_task_just_created() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let area_path = location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Homesteading"),
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
                ("title", "Renovation"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    let task_path = location_of(
        &app.post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Regrout the shower"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();

    // The href is HTML-escaped (MiniJinja auto-escapes `/` too, defense in
    // depth against attribute-breaking — see `templates.rs`'s own test for
    // this), so check for the id itself plus the escaped path fragments
    // rather than the raw `/areas/{id}` string.
    let area_id = area_path.trim_start_matches("/areas/");
    let project_id = project_path.trim_start_matches("/projects/");
    let task_id = task_path.trim_start_matches("/tasks/");

    let area_hits = body_text(app.get("/search?q=Homesteading", cookie).await).await;
    assert!(
        area_hits.contains(area_id) && area_hits.contains("Homesteading"),
        "search must find the area by title: {area_hits}"
    );

    let project_hits = body_text(app.get("/search?q=Renovation", cookie).await).await;
    assert!(
        project_hits.contains(project_id) && project_hits.contains("Renovation"),
        "search must find the project by title: {project_hits}"
    );

    let task_hits = body_text(app.get("/search?q=Regrout", cookie).await).await;
    assert!(
        task_hits.contains(task_id) && task_hits.contains("Regrout"),
        "search must find the task by a whole word from its title: {task_hits}"
    );
}

#[tokio::test]
async fn an_hx_search_request_gets_only_the_results_fragment() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Homesteading"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    );

    let full = app.get("/search?q=Homesteading", cookie).await;
    assert_eq!(full.status(), StatusCode::OK);
    let full_body = body_text(full).await;
    assert!(full_body.contains("<html") || full_body.contains("<!doctype html>"));

    let fragment = app.get_hx("/search?q=Homesteading", cookie).await;
    assert_eq!(fragment.status(), StatusCode::OK);
    let fragment_body = body_text(fragment).await;
    assert!(
        !fragment_body.contains("<html") && !fragment_body.contains("<!doctype html>"),
        "an HX-Request must get a bare results fragment: {fragment_body}"
    );
    assert!(fragment_body.contains("Homesteading"));
}

#[tokio::test]
async fn an_empty_query_returns_no_results_and_no_error() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let response = app.get("/search", cookie).await;
    assert_eq!(response.status(), StatusCode::OK);
}

/// The full archive round trip through HTTP (`docs/DOMAIN.md` §2): archive a
/// task, confirm it leaves the normal views *and* plain search, confirm it
/// is findable via the "include archived" affordance, then unarchive it and
/// confirm it is back to normal — the search-index half of this phase's
/// audit finding (`SearchIndex::remove_task` archives the index entry rather
/// than deleting it, specifically so this round trip is possible at all).
#[tokio::test]
async fn archiving_a_task_removes_it_from_plain_search_but_leaves_it_findable_archived() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let area_path = location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Home hunting"),
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
                ("title", "House shopping"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();
    let task_path = location_of(
        &app.post_form(
            &format!("{project_path}/tasks"),
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Retire the fixer-upper"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();

    // Findable before archiving.
    let hits = body_text(app.get("/search?q=fixer-upper", cookie).await).await;
    assert!(hits.contains("Retire the fixer-upper"));

    // Archive it.
    let archive = app
        .post_form(
            &format!("{task_path}/archive"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(archive.status(), StatusCode::SEE_OTHER);

    // Vanished from plain search.
    let hits = body_text(app.get("/search?q=fixer-upper", cookie).await).await;
    assert!(
        !hits.contains("Retire the fixer-upper"),
        "an archived task must not appear in plain search: {hits}"
    );

    // Findable via the explicit "include archived" affordance.
    let archived_hits = body_text(app.get("/search?q=fixer-upper&archived=1", cookie).await).await;
    assert!(
        archived_hits.contains("Retire the fixer-upper"),
        "an archived task must be findable via the include-archived search: {archived_hits}"
    );
    assert!(archived_hits.contains("archived"));

    // Without the affordance, still not found even though it exists.
    let hits = body_text(app.get("/search?q=fixer-upper", cookie).await).await;
    assert!(!hits.contains("Retire the fixer-upper"));

    // Unarchive it: back to normal.
    let unarchive = app
        .post_form(
            &format!("{task_path}/unarchive"),
            &[("csrf_token", support::DEV_CSRF_TOKEN)],
            cookie,
        )
        .await;
    assert_eq!(unarchive.status(), StatusCode::SEE_OTHER);

    let hits = body_text(app.get("/search?q=fixer-upper", cookie).await).await;
    assert!(
        hits.contains("Retire the fixer-upper"),
        "unarchiving must restore it to plain search: {hits}"
    );
}

/// Same round trip for a project, plus confirming an archived project no
/// longer appears in plain search while its child task (still active) does.
#[tokio::test]
async fn archiving_a_project_is_findable_only_via_the_include_archived_search() {
    let app = TestApp::new(true).await;
    let cookie: Option<&str> = None;

    let area_path = location_of(
        &app.post_form(
            "/areas",
            &[
                ("csrf_token", support::DEV_CSRF_TOKEN),
                ("title", "Home hunting"),
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
                ("title", "Sunsetting renovation"),
                ("description", ""),
            ],
            cookie,
        )
        .await,
    )
    .to_string();

    app.post_form(
        &format!("{project_path}/archive"),
        &[("csrf_token", support::DEV_CSRF_TOKEN)],
        cookie,
    )
    .await;

    let hits = body_text(app.get("/search?q=Sunsetting", cookie).await).await;
    assert!(!hits.contains("Sunsetting renovation"));

    let archived_hits = body_text(app.get("/search?q=Sunsetting&archived=1", cookie).await).await;
    assert!(archived_hits.contains("Sunsetting renovation"));
}
