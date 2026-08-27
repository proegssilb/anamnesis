//! Builds the `axum::Router`: one resource-oriented route per job, so a
//! future PWA client can call the same URLs and content-negotiate JSON
//! instead of HTML.

use axum::Router;
use axum::routing::{get, post};

use crate::handlers::{
    accept_suggestion_handler, accept_tangle_offer_handler, add_comment_handler,
    add_field_definition_handler, add_file_attachment_handler, add_link_attachment_handler,
    archive_all_handler, archive_project_handler, archive_task_handler, callback_handler,
    create_area_handler, create_project_handler, create_relationship_handler, create_task_handler,
    delete_relationship_handler, download_attachment_handler, drop_tangle_handler,
    drop_task_handler, edit_area_handler, edit_task_handler, healthz_handler, list_areas_handler,
    login_handler, logout_handler, raise_task_handler, reposition_handler, root_handler,
    search_handler, set_field_value_handler, set_parent_handler, transition_project_status_handler,
    unarchive_project_handler, unarchive_task_handler, update_settings_handler, view_area_handler,
    view_board_handler, view_project_handler, view_settings_handler, view_task_handler,
};
use crate::state::AppState;
use crate::static_files;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz_handler))
        .route("/", get(root_handler))
        .route("/login", get(login_handler))
        .route("/auth/callback", get(callback_handler))
        .route("/logout", post(logout_handler))
        .route("/areas", get(list_areas_handler).post(create_area_handler))
        .route(
            "/areas/{id}",
            get(view_area_handler).post(edit_area_handler),
        )
        .route("/areas/{id}/projects", post(create_project_handler))
        .route("/projects/{id}", get(view_project_handler))
        .route(
            "/projects/{id}/status",
            post(transition_project_status_handler),
        )
        .route("/projects/{id}/archive", post(archive_project_handler))
        .route("/projects/{id}/unarchive", post(unarchive_project_handler))
        .route("/projects/{id}/fields", post(add_field_definition_handler))
        .route("/projects/{id}/tasks", post(create_task_handler))
        .route(
            "/tasks/{id}",
            get(view_task_handler).post(edit_task_handler),
        )
        .route("/tasks/{id}/raise", post(raise_task_handler))
        .route("/tasks/{id}/drop", post(drop_task_handler))
        .route("/tasks/{id}/archive", post(archive_task_handler))
        .route("/tasks/{id}/unarchive", post(unarchive_task_handler))
        .route("/tasks/{id}/parent", post(set_parent_handler))
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
            "/tasks/{id}/fields/{field_id}",
            post(set_field_value_handler),
        )
        .route("/board", get(view_board_handler))
        .route("/board/reposition", post(reposition_handler))
        .route("/board/suggestion/accept", post(accept_suggestion_handler))
        .route(
            "/board/suggestion/accept-tangle",
            post(accept_tangle_offer_handler),
        )
        .route("/board/archive-all", post(archive_all_handler))
        .route("/tangles/{id}/drop", post(drop_tangle_handler))
        .route("/search", get(search_handler))
        .route(
            "/settings",
            get(view_settings_handler).post(update_settings_handler),
        )
        .route("/static/app.css", get(static_files::app_css))
        .route("/static/app.js", get(static_files::app_js))
        .route("/static/htmx.min.js", get(static_files::htmx_js))
        .route("/static/sortable.min.js", get(static_files::sortable_js))
        .route("/static/icon.svg", get(static_files::icon))
        .route("/manifest.webmanifest", get(static_files::manifest))
        .with_state(state)
}
