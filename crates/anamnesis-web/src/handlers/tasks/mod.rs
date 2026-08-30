//! Task detail: fields, relationships, checklist, comments, attachments
//! (`docs/DOMAIN.md` §8) — plus raising a task above the horizon and
//! dropping it back (§2, §5's bounce accounting).
//!
//! Split by concern rather than kept as one file: [`page`] assembles and
//! renders the task detail page itself (shared by every mutating handler
//! below it that needs to re-render on success or on a rule violation);
//! [`view`], [`edit`], [`lifecycle`], [`comments`], [`attachments`],
//! [`hierarchy`], [`relationships`], [`fields`] each own one slice of the
//! page's mutating routes; [`candidates`] is the title-search engine shared
//! by the parent and relationship pickers in [`hierarchy`] and
//! [`relationships`].

mod attachments;
mod candidates;
mod comments;
mod edit;
mod fields;
mod hierarchy;
mod lifecycle;
mod page;
mod relationships;
mod view;

pub use attachments::{
    add_file_attachment_handler, add_link_attachment_handler, download_attachment_handler,
};
pub use comments::add_comment_handler;
pub use edit::{edit_task_description_handler, edit_task_title_handler};
pub use fields::set_field_value_handler;
pub use hierarchy::{add_checklist_item_handler, parent_candidates_handler, set_parent_handler};
pub use lifecycle::{
    archive_task_handler, drop_task_handler, raise_task_handler, unarchive_task_handler,
};
pub use relationships::{
    create_relationship_handler, delete_relationship_handler, relationship_candidates_handler,
};
pub use view::view_task_handler;

use anamnesis_app::AppError;
use anamnesis_core::TaskId;

use crate::error::WebError;
use crate::state::AppState;

/// Resolves the role a task's own project grants `user` — every task
/// handler needs this once, up front, to gate the actual use case call.
pub(super) async fn role_for_task(
    state: &AppState,
    user_id: &anamnesis_core::UserId,
    task_id: TaskId,
) -> Result<
    (
        anamnesis_core::ProjectId,
        Option<anamnesis_core::policy::Role>,
    ),
    WebError,
> {
    let aggregate = state.tasks.load(task_id).await?.ok_or(AppError::NotFound)?;
    let project_id = aggregate.task.project_id;
    let project = state
        .projects
        .load(project_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let role =
        super::access::project_role(state, user_id, project_id, project.project.area_id).await?;
    Ok((project_id, role))
}

/// The task detail page's own read-side calls (`list_comments`,
/// `list_attachments`) are gated identically to `ViewTask`
/// (`can_view_project`), and by the time `render_task_page` runs, the
/// caller has already succeeded at a stronger check on this exact task
/// (`view_task`/`edit_task`/... all resolve the real effective role and
/// would have failed already) — so a fixed `Member` placeholder here only
/// ever satisfies a gate the caller already cleared, never substitutes for
/// it.
pub(super) fn member_role() -> anamnesis_core::policy::Role {
    anamnesis_core::policy::Role::Member
}
