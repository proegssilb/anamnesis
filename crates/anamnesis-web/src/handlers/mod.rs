//! Request handlers, one small file per resource. Every mutating handler
//! follows the same shape: a thin `pub async fn ..._handler` axum can route
//! to, delegating to a private `..._impl` that returns
//! `Result<Response, WebError>` so it can use `?`; the thin wrapper turns an
//! `Err` into a rendered error page via [`crate::error::WebError::into_response_with`].

mod access;
mod areas;
mod board;
mod field_form;
mod format;
mod forms;
mod login;
mod membership;
mod misc;
mod projects;
mod search;
mod settings;
mod tasks;

pub use areas::{
    create_area_handler, create_project_handler, edit_area_handler, list_areas_handler,
    transition_project_status_handler, view_area_handler,
};
pub use board::{
    accept_suggestion_handler, accept_tangle_offer_handler, archive_all_handler,
    drop_tangle_handler, reposition_handler, view_board_handler,
};
pub use login::{callback_handler, login_handler, logout_handler};
pub use membership::{
    grant_area_member_handler, grant_project_member_handler, grant_system_admin_handler,
    revoke_area_member_handler, revoke_project_member_handler, revoke_system_admin_handler,
    view_users_handler,
};
pub use misc::{healthz_handler, root_handler};
pub use projects::{
    add_field_definition_handler, archive_project_handler, create_task_handler,
    list_projects_handler, unarchive_project_handler, view_project_handler,
};
pub use search::search_handler;
pub use settings::{update_settings_handler, view_settings_handler};
pub use tasks::{
    add_checklist_item_handler, add_comment_handler, add_file_attachment_handler,
    add_link_attachment_handler, archive_task_handler, create_relationship_handler,
    delete_relationship_handler, download_attachment_handler, drop_task_handler,
    edit_task_description_handler, edit_task_title_handler, parent_candidates_handler,
    raise_task_handler, set_field_value_handler, set_parent_handler, unarchive_task_handler,
    view_task_handler,
};
