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

/// Drops a task below the horizon, first reading whether it was leaving a
/// Done column so the bounce count stays honest (`docs/DOMAIN.md` §5).
/// Shared by [`lifecycle::drop_task_impl`] and
/// [`crate::handlers::projects::drop_project_task_impl`] — the drag target
/// for dropping a card back off a project's board.
pub(super) async fn drop_task_with_bounce_accounting(
    state: &AppState,
    role: Option<anamnesis_core::policy::Role>,
    task_id: TaskId,
) -> Result<(), WebError> {
    let aggregate = state.tasks.load(task_id).await?.ok_or(AppError::NotFound)?;
    let left_a_done_column = match aggregate.task.placement {
        anamnesis_core::Placement::OnBoard { column, .. } => {
            let columns = state.board.columns_with_items().await?;
            super::format::column_is_done(&columns, column).unwrap_or(false)
        }
        anamnesis_core::Placement::Below => false,
    };

    anamnesis_app::drop_task(
        state.tasks.as_ref(),
        state.clock.as_ref(),
        role,
        task_id,
        left_a_done_column,
    )
    .await?;
    Ok(())
}

/// The one message every caller of [`raise_task_to_column`] shows when a
/// raise is refused, kept in one place so the task page and the project
/// page cannot drift apart on the wording.
pub(super) const WIP_LIMIT_MESSAGE: &str = "That column is already at its work-in-progress limit.";

/// Whether a raise landed, or was refused because the destination column
/// was already full — see [`raise_task_to_column`].
pub(super) enum RaiseOutcome {
    Raised,
    WipLimitExceeded,
}

/// Raises a task to the end of `column`, reading the column's current
/// occupancy to find that end. Shared by [`lifecycle::raise_task_impl`] and
/// [`crate::handlers::projects::raise_project_task_impl`] — the drag target
/// for raising a card onto a project's board — which differ only in how
/// they choose the column and how they render the two outcomes.
///
/// `WipLimitExceeded` comes back as an `Ok` variant rather than an error
/// because it is not one: every caller renders it as a message on its own
/// page (or, on an hx path, silently reverts), and none of them propagate
/// it. Every *other* `AppError` is still a genuine failure and propagates.
pub(super) async fn raise_task_to_column(
    state: &AppState,
    role: Option<anamnesis_core::policy::Role>,
    task_id: TaskId,
    column: anamnesis_core::ColumnId,
) -> Result<RaiseOutcome, WebError> {
    let position = state.board.count_on_column(column).await?;
    match anamnesis_app::raise_task(
        state.tasks.as_ref(),
        state.board.as_ref(),
        state.clock.as_ref(),
        role,
        task_id,
        column,
        position,
    )
    .await
    {
        Ok(_) => Ok(RaiseOutcome::Raised),
        Err(AppError::WipLimitExceeded) => Ok(RaiseOutcome::WipLimitExceeded),
        Err(err) => Err(WebError::from(err)),
    }
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
