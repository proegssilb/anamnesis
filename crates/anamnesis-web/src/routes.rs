//! Builds the `axum::Router`: one resource-oriented route per job, so a
//! future PWA client can call the same URLs and content-negotiate JSON
//! instead of HTML.
//!
//! Routes are grouped below by the resource they act on (areas, projects,
//! tasks, the board, site-wide/admin concerns, static assets) and merged
//! into one router in [`build_router`]. Each group function is a plain
//! `Router<AppState>` — only `build_router` supplies the state, since
//! `Router::merge` requires every side to already share the same state type.

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};

use crate::handlers::{
    accept_suggestion_handler, accept_tangle_offer_handler, add_checklist_item_handler,
    add_comment_handler, add_field_definition_handler, add_file_attachment_handler,
    add_link_attachment_handler, archive_all_handler, archive_project_handler,
    archive_task_handler, callback_handler, create_area_handler, create_project_handler,
    create_relationship_handler, create_task_handler, delete_relationship_handler,
    download_attachment_handler, drop_project_task_handler, drop_tangle_handler, drop_task_handler,
    edit_area_handler, edit_task_description_handler, edit_task_title_handler,
    grant_admin_group_handler, grant_area_group_handler, grant_area_member_handler,
    grant_project_group_handler, grant_project_member_handler, grant_system_admin_handler,
    healthz_handler, list_areas_handler, list_projects_handler, login_handler, logout_handler,
    parent_candidates_handler, raise_project_task_handler, raise_task_handler,
    relationship_candidates_handler, reposition_handler, revoke_admin_group_handler,
    revoke_area_group_handler, revoke_area_member_handler, revoke_project_group_handler,
    revoke_project_member_handler, revoke_system_admin_handler, root_handler, search_handler,
    set_field_value_handler, set_parent_handler, transition_project_status_handler,
    unarchive_project_handler, unarchive_task_handler, update_settings_handler, view_area_handler,
    view_board_handler, view_project_handler, view_settings_handler, view_task_handler,
    view_users_handler,
};
use crate::state::AppState;
use crate::static_files;

pub fn build_router(state: AppState) -> Router {
    // Read before `state` is moved into `with_state`.
    let max_body_bytes = state.max_body_bytes;
    Router::new()
        .merge(area_routes())
        .merge(project_routes())
        .merge(task_routes())
        .merge(board_routes())
        .merge(admin_routes())
        .merge(static_routes())
        // Replaces axum's own 2 MiB `DefaultBodyLimit`, which rejects most
        // real file attachments (`docs/DOMAIN.md` §3) long before
        // `add_file_attachment_handler` ever runs.
        .layer(DefaultBodyLimit::max(max_body_bytes))
        .with_state(state)
}

/// The area grid and, per area, its own detail page plus the members and
/// child-project actions hung off it (`docs/DOMAIN.md` §3).
fn area_routes() -> Router<AppState> {
    Router::new()
        .route("/areas", get(list_areas_handler).post(create_area_handler))
        .route(
            "/areas/{id}",
            get(view_area_handler).post(edit_area_handler),
        )
        .route("/areas/{id}/projects", post(create_project_handler))
        .route("/areas/{id}/members", post(grant_area_member_handler))
        .route(
            "/areas/{id}/members/revoke",
            post(revoke_area_member_handler),
        )
        .route("/areas/{id}/member-groups", post(grant_area_group_handler))
        .route(
            "/areas/{id}/member-groups/revoke",
            post(revoke_area_group_handler),
        )
}

/// The project listing plus a single project's own lifecycle: membership,
/// status transitions, archival, custom fields, and the tasks created under
/// it.
fn project_routes() -> Router<AppState> {
    Router::new()
        .route("/projects", get(list_projects_handler))
        .route("/projects/{id}", get(view_project_handler))
        .route("/projects/{id}/members", post(grant_project_member_handler))
        .route(
            "/projects/{id}/members/revoke",
            post(revoke_project_member_handler),
        )
        .route(
            "/projects/{id}/member-groups",
            post(grant_project_group_handler),
        )
        .route(
            "/projects/{id}/member-groups/revoke",
            post(revoke_project_group_handler),
        )
        .route(
            "/projects/{id}/status",
            post(transition_project_status_handler),
        )
        .route("/projects/{id}/archive", post(archive_project_handler))
        .route("/projects/{id}/unarchive", post(unarchive_project_handler))
        .route("/projects/{id}/fields", post(add_field_definition_handler))
        .route("/projects/{id}/tasks", post(create_task_handler))
        .route(
            "/projects/{id}/tasks/{task_id}/raise",
            post(raise_project_task_handler),
        )
        .route(
            "/projects/{id}/tasks/{task_id}/drop",
            post(drop_project_task_handler),
        )
}

/// A single task's own page and everything scoped to it: editing, lifecycle,
/// hierarchy, checklists, comments, attachments (and their download route),
/// relationships, and custom field values.
fn task_routes() -> Router<AppState> {
    Router::new()
        .route("/tasks/{id}", get(view_task_handler))
        .route("/tasks/{id}/title", post(edit_task_title_handler))
        .route(
            "/tasks/{id}/description",
            post(edit_task_description_handler),
        )
        .route("/tasks/{id}/raise", post(raise_task_handler))
        .route("/tasks/{id}/drop", post(drop_task_handler))
        .route("/tasks/{id}/archive", post(archive_task_handler))
        .route("/tasks/{id}/unarchive", post(unarchive_task_handler))
        .route("/tasks/{id}/parent", post(set_parent_handler))
        .route(
            "/tasks/{id}/parent-candidates",
            get(parent_candidates_handler),
        )
        .route("/tasks/{id}/children", post(add_checklist_item_handler))
        .route("/tasks/{id}/comments", post(add_comment_handler))
        .route("/tasks/{id}/attachments", post(add_link_attachment_handler))
        .route(
            "/tasks/{id}/attachments/file",
            post(add_file_attachment_handler),
        )
        .route(
            "/attachments/{id}/download",
            get(download_attachment_handler),
        )
        .route(
            "/tasks/{id}/relationships",
            post(create_relationship_handler),
        )
        .route(
            "/tasks/{id}/relationships/{relationship_id}/delete",
            post(delete_relationship_handler),
        )
        .route(
            "/tasks/{id}/relationship-candidates",
            get(relationship_candidates_handler),
        )
        .route(
            "/tasks/{id}/fields/{field_id}",
            post(set_field_value_handler),
        )
}

/// The global task board (`docs/DOMAIN.md` §8): its view, drag-and-drop
/// reposition, tangle-suggestion resolution, and bulk archive.
fn board_routes() -> Router<AppState> {
    Router::new()
        .route("/board", get(view_board_handler))
        .route("/board/reposition", post(reposition_handler))
        .route("/board/suggestion/accept", post(accept_suggestion_handler))
        .route(
            "/board/suggestion/accept-tangle",
            post(accept_tangle_offer_handler),
        )
        .route("/board/archive-all", post(archive_all_handler))
        .route("/tangles/{id}/drop", post(drop_tangle_handler))
}

/// Cross-cutting, not-a-single-resource concerns: health, the home page,
/// the OIDC login round trip, global search, app settings, and System Admin
/// grants.
fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz_handler))
        .route("/", get(root_handler))
        .route("/login", get(login_handler))
        .route("/auth/callback", get(callback_handler))
        .route("/logout", post(logout_handler))
        .route("/search", get(search_handler))
        .route(
            "/settings",
            get(view_settings_handler).post(update_settings_handler),
        )
        .route(
            "/users",
            get(view_users_handler).post(grant_system_admin_handler),
        )
        .route("/users/revoke", post(revoke_system_admin_handler))
        .route("/users/groups", post(grant_admin_group_handler))
        .route("/users/groups/revoke", post(revoke_admin_group_handler))
}

/// Static assets served directly out of the binary (`crate::static_files`).
fn static_routes() -> Router<AppState> {
    Router::new()
        .route("/static/app.css", get(static_files::app_css))
        .route("/static/app.js", get(static_files::app_js))
        .route("/static/htmx.min.js", get(static_files::htmx_js))
        .route("/static/sortable.min.js", get(static_files::sortable_js))
        .route("/static/icon.svg", get(static_files::icon))
        .route("/manifest.webmanifest", get(static_files::manifest))
}
