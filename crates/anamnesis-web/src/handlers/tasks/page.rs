//! Assembles and renders the task detail page: the task itself, its
//! checklist children, comments, attachments, relationships (with the other
//! end's title resolved for display), and the board columns available for
//! the raise-task form. Every other module in [`super`] that mutates a task
//! and needs to re-render it (on success or on a rule violation) goes
//! through [`render_task_page`].

use std::collections::HashMap;

use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use minijinja::context;

use anamnesis_app::{
    Attachment, BoardColumn, Comment, list_attachments, list_comments, resolve_kind,
};
use anamnesis_core::{Placement, UserId};

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::state::AppState;

use crate::handlers::format::{
    column_is_done, column_title, field_input_value, format_field_data, format_field_kind,
};

use super::member_role;

#[allow(clippy::too_many_arguments)]
pub(super) async fn render_task_page(
    state: &AppState,
    user: &CurrentUser,
    task_id: anamnesis_core::TaskId,
    task: &anamnesis_core::Task,
    error: Option<&str>,
    open_hint: Option<&str>,
    relationship_prefill: Option<minijinja::Value>,
    status: StatusCode,
) -> Result<Response, WebError> {
    let children = state.tasks.list_children(task_id).await?;
    let comments = list_comments(state.comments.as_ref(), Some(member_role()), task_id).await?;
    let comments_ctx = build_comments_context(state, &comments, &user.user_id).await?;
    let (attachments, relationships, parent) =
        build_related_context(state, task_id, task.parent_task_id).await?;

    let (column_options, current_column_is_done, current_column_title, children_ctx) =
        build_board_context(state, task, &children).await?;
    let (fields, project_title, mentionable_users) =
        build_project_context(state, user, task).await?;
    let description_html =
        crate::handlers::markdown::render(&task.description, user.user_id.as_str());

    let tmpl = state
        .templates
        .get_template("task.html")
        .map_err(WebError::template)?;
    let ctx = task_page_context(TaskPageContext {
        task,
        description_html,
        project_title,
        mentionable_users,
        current_column_is_done,
        current_column_title,
        open_hint,
        relationship_prefill,
        parent,
        children_ctx,
        comments_ctx,
        attachments,
        relationships,
        column_options,
        fields,
        user,
        error,
        max_body_bytes: state.max_body_bytes,
    });
    let body = tmpl.render(ctx).map_err(WebError::template)?;
    Ok((status, Html(body)).into_response())
}

/// Everything [`task_page_context`] needs to build `task.html`'s template
/// context — one value per template variable, gathered by [`render_task_page`]
/// from several independent sources (the task itself, its board placement,
/// comments, attachments, relationships...) but rendered as a single unit.
struct TaskPageContext<'a> {
    task: &'a anamnesis_core::Task,
    description_html: minijinja::Value,
    project_title: Option<String>,
    mentionable_users: minijinja::Value,
    current_column_is_done: Option<bool>,
    current_column_title: Option<String>,
    open_hint: Option<&'a str>,
    relationship_prefill: Option<minijinja::Value>,
    parent: Option<minijinja::Value>,
    children_ctx: Vec<minijinja::Value>,
    comments_ctx: Vec<minijinja::Value>,
    attachments: Vec<Attachment>,
    relationships: Vec<minijinja::Value>,
    column_options: Vec<minijinja::Value>,
    fields: Vec<minijinja::Value>,
    user: &'a CurrentUser,
    error: Option<&'a str>,
    max_body_bytes: usize,
}

/// Assembles `task.html`'s template context — split out of
/// [`render_task_page`] purely so that function's own async data-gathering
/// stays under `just lizard`'s length budget; this half does no I/O of its
/// own, just enumeration.
fn task_page_context(ctx: TaskPageContext) -> minijinja::Value {
    context! {
        task => ctx.task,
        description_html => ctx.description_html,
        project_title => ctx.project_title,
        mentionable_users => ctx.mentionable_users,
        is_on_board => ctx.task.placement.is_on_board(),
        current_column_is_done => ctx.current_column_is_done,
        current_column_title => ctx.current_column_title,
        open_hint => ctx.open_hint,
        relationship_prefill => ctx.relationship_prefill,
        parent => ctx.parent,
        children => ctx.children_ctx,
        comments => ctx.comments_ctx,
        attachments => ctx.attachments,
        relationships => ctx.relationships,
        column_options => ctx.column_options,
        fields => ctx.fields,
        csrf_token => ctx.user.csrf_token,
        current_user => ctx.user.display_name,
        error => ctx.error,
        max_body_bytes => ctx.max_body_bytes,
    }
}

/// Builds the "Relationships" section's context: each edge's label (forward
/// or reverse, depending on which end `task_id` is), and the other end's
/// title (or a placeholder if that task has since been deleted). Split out
/// of [`render_task_page`] — this is the one part of the page that needs a
/// database round trip per relationship.
async fn build_relationships_context(
    state: &AppState,
    task_id: anamnesis_core::TaskId,
) -> Result<Vec<minijinja::Value>, WebError> {
    let raw_relationships = state.relationships.list_for_task(task_id).await?;
    let mut relationships = Vec::with_capacity(raw_relationships.len());
    for rel in &raw_relationships {
        let (other_id, forward) = if rel.from_task_id == task_id {
            (rel.to_task_id, true)
        } else {
            (rel.from_task_id, false)
        };
        let kind = resolve_kind(state.projects.as_ref(), rel.kind_id).await?;
        let label = if forward {
            kind.forward_label.as_str().to_string()
        } else {
            kind.reverse_label.as_str().to_string()
        };
        let other_title = state
            .tasks
            .load(other_id)
            .await?
            .map(|a| a.task.title.as_str().to_string())
            .unwrap_or_else(|| "(deleted task)".to_string());
        relationships.push(context! {
            id => rel.id.to_string(),
            label => label,
            other_id => other_id.to_string(),
            other_title => other_title,
        });
    }
    Ok(relationships)
}

/// Each comment's author, resolved to its cached display name when known —
/// otherwise issue #38 stands: a comment's author was only ever renderable
/// as its raw OIDC `sub`, which is what `anamnesis_app::UserDirectoryQuery`
/// exists to fix. An author with no cached entry (never logged in since it
/// was added) falls back to that same raw id, exactly as before.
///
/// A comment imported from an external issue tracker (issues #40/#41,
/// `Comment::origin`) never has a cached display name — its `author` is a
/// synthetic id, not a real logged-in user — so its origin's own
/// `external_author_display` is shown instead, alongside a link back to
/// the original comment.
async fn build_comments_context(
    state: &AppState,
    comments: &[Comment],
    viewer: &UserId,
) -> Result<Vec<minijinja::Value>, WebError> {
    let authors: Vec<UserId> = comments
        .iter()
        .filter(|c| c.origin.is_none())
        .map(|c| c.author.clone())
        .collect();
    let names: HashMap<UserId, String> = state.user_directory.display_names(&authors).await?;
    Ok(comments
        .iter()
        .map(|c| build_one_comment_context(c, &names, viewer))
        .collect())
}

fn build_one_comment_context(
    c: &Comment,
    names: &HashMap<UserId, String>,
    viewer: &UserId,
) -> minijinja::Value {
    let author_name = match &c.origin {
        Some(origin) => origin.external_author_display.clone(),
        None => names
            .get(&c.author)
            .cloned()
            .unwrap_or_else(|| c.author.to_string()),
    };
    context! {
        id => c.id.to_string(),
        author_name => author_name,
        body_html => crate::handlers::markdown::render(c.body.as_str(), viewer.as_str()),
        origin => c.origin.as_ref().map(build_comment_origin_context),
    }
}

/// The provider label and link-back the task page shows beneath an
/// imported comment (issues #40/#41).
fn build_comment_origin_context(origin: &anamnesis_app::CommentOrigin) -> minijinja::Value {
    let provider_label = match origin.provider {
        anamnesis_app::SyncProvider::GitHub => "GitHub",
        anamnesis_app::SyncProvider::Forgejo => "Forgejo",
    };
    context! {
        provider_label => provider_label,
        external_url => origin.external_url,
    }
}

/// The checklist parent's display context — resolved the same way a
/// relationship's other end is resolved in
/// [`build_relationships_context`] — used both for the status-row badge and
/// the "Parent task" section, so a child task's page actually shows it is
/// one rather than leaving that only visible from the parent's own
/// checklist.
async fn build_parent_context(
    state: &AppState,
    parent_task_id: Option<anamnesis_core::TaskId>,
) -> Result<Option<minijinja::Value>, WebError> {
    let Some(parent_id) = parent_task_id else {
        return Ok(None);
    };
    let title = state
        .tasks
        .load(parent_id)
        .await?
        .map(|a| a.task.title.as_str().to_string())
        .unwrap_or_else(|| "(deleted task)".to_string());
    Ok(Some(
        context! { id => parent_id.to_string(), title => title },
    ))
}

/// The three remaining "detail column" reads [`render_task_page`] needs
/// beyond comments: attachments, relationships, and the checklist parent.
/// Grouped only because none of them individually earns its own line in
/// the caller — each is already its own well-named call — not because they
/// share a data source the way [`build_board_context`]'s do.
async fn build_related_context(
    state: &AppState,
    task_id: anamnesis_core::TaskId,
    parent_task_id: Option<anamnesis_core::TaskId>,
) -> Result<
    (
        Vec<Attachment>,
        Vec<minijinja::Value>,
        Option<minijinja::Value>,
    ),
    WebError,
> {
    let attachments =
        list_attachments(state.attachments.as_ref(), Some(member_role()), task_id).await?;
    let relationships = build_relationships_context(state, task_id).await?;
    let parent = build_parent_context(state, parent_task_id).await?;
    Ok((attachments, relationships, parent))
}

/// Everything on the page that reads from the board's current column list:
/// the raise-task form's choices, this task's own placement pill
/// (`is_done`/title of the column it's in, if any), and each checklist
/// child's `done` reading. Split out of [`render_task_page`] the same way
/// [`build_project_context`] is — one repository read
/// (`columns_with_items`) feeding several display-only derivations that
/// have nothing to do with each other except sharing that read.
async fn build_board_context(
    state: &AppState,
    task: &anamnesis_core::Task,
    children: &[anamnesis_core::Task],
) -> Result<
    (
        Vec<minijinja::Value>,
        Option<bool>,
        Option<String>,
        Vec<minijinja::Value>,
    ),
    WebError,
> {
    let columns = state.board.columns_with_items().await?;
    let column_options = build_column_options(&columns);
    let (current_column_is_done, current_column_title) =
        current_column_info(&columns, task.placement);
    let children_ctx = build_children_context(children, &columns);
    Ok((
        column_options,
        current_column_is_done,
        current_column_title,
        children_ctx,
    ))
}

/// The raise-task form's column choices: every board column's id and title,
/// in board order.
fn build_column_options(columns: &[BoardColumn]) -> Vec<minijinja::Value> {
    columns
        .iter()
        .map(|c| context! { id => c.column.id.to_string(), title => c.column.title.as_str() })
        .collect()
}

/// Where a task currently sits on the board, for the status-row pill:
/// `(is_done, column_title)`, both `None` for a task that has dropped below
/// the horizon (`Placement::Below`). The one place `render_task_page` needs
/// to match on `Placement` for this — [`build_children_context`] below has
/// its own, narrower need (just a `done` bool) and is intentionally kept
/// separate rather than sharing this tuple shape.
fn current_column_info(
    columns: &[BoardColumn],
    placement: Placement,
) -> (Option<bool>, Option<String>) {
    match placement {
        Placement::OnBoard { column, .. } => (
            column_is_done(columns, column),
            column_title(columns, column),
        ),
        Placement::Below => (None, None),
    }
}

/// Each checklist child's card context: its title and the same "on the
/// board, in an `is_done` column" `done` reading the parent task's own
/// status pill uses — checked purely for display here, distinct from
/// actually completing anything.
fn build_children_context(
    children: &[anamnesis_core::Task],
    columns: &[BoardColumn],
) -> Vec<minijinja::Value> {
    children
        .iter()
        .map(|c| {
            let done = match c.placement {
                Placement::OnBoard { column, .. } => {
                    column_is_done(columns, column).unwrap_or(false)
                }
                Placement::Below => false,
            };
            context! {
                id => c.id.to_string(),
                title => c.title.as_str(),
                done => done,
            }
        })
        .collect()
}

/// One field definition's template context: its stored value (if any) in
/// this task's `field_values`, rendered both for display
/// ([`format_field_data`]) and as a form prefill ([`field_input_value`]).
/// Split out of [`build_fields_context`] so the `map` there reads as one
/// step, not an inline closure doing the lookup and the rendering both.
fn field_context(
    state: &AppState,
    def: &anamnesis_core::FieldDefinition,
    field_values: &[anamnesis_core::FieldValue],
) -> minijinja::Value {
    let stored = field_values.iter().find(|v| v.field_id == def.id);
    let (input_value, currency_code) = stored
        .map(|v| field_input_value(&v.data, state.timezone.as_ref(), &state.timezone_name))
        .unwrap_or_default();
    context! {
        id => def.id.to_string(),
        name => def.name.as_str(),
        kind => format_field_kind(def.kind),
        show_on_card => def.show_on_card,
        display_value => stored.map(|v| format_field_data(&v.data)),
        input_value => input_value,
        currency_code => currency_code.unwrap_or_default(),
    }
}

/// Custom field definitions + this task's own values (`docs/DOMAIN.md` §3):
/// the section that made every field genuinely editable, not just displayed
/// (`super::field_form`'s module doc comment).
async fn build_fields_context(
    state: &AppState,
    task: &anamnesis_core::Task,
    field_definitions: &[anamnesis_core::FieldDefinition],
) -> Result<Vec<minijinja::Value>, WebError> {
    let field_values = state
        .tasks
        .load(task.id)
        .await?
        .map(|a| a.field_values)
        .unwrap_or_default();
    Ok(field_definitions
        .iter()
        .map(|def| field_context(state, def, &field_values))
        .collect())
}

/// The task's parent project, loaded once and read for both the fields
/// section (its field definitions) and the breadcrumb (its title) — the one
/// project-level lookup [`render_task_page`] needs, split out the same way
/// [`build_relationships_context`] and [`build_parent_context`] are.
async fn build_project_context(
    state: &AppState,
    user: &CurrentUser,
    task: &anamnesis_core::Task,
) -> Result<(Vec<minijinja::Value>, Option<String>, minijinja::Value), WebError> {
    let project = state.projects.load(task.project_id).await?;
    let field_definitions = project
        .as_ref()
        .map(|a| a.field_definitions.as_slice())
        .unwrap_or_default();
    let fields = build_fields_context(state, task, field_definitions).await?;
    let area_id = project.as_ref().map(|a| a.project.area_id);
    let project_title = project.map(|a| a.project.title.as_str().to_string());
    let mentionable_users = build_mentionable_users(state, user, task.project_id, area_id).await?;
    Ok((fields, project_title, mentionable_users))
}

/// The `@`-mention picker's candidate list for this task's project — empty
/// (not an error) when the project has since been deleted, matching how
/// [`build_project_context`] already degrades `project_title` to `None` in
/// that same case rather than failing the whole page.
async fn build_mentionable_users(
    state: &AppState,
    user: &CurrentUser,
    project_id: anamnesis_core::ProjectId,
    area_id: Option<anamnesis_core::AreaId>,
) -> Result<minijinja::Value, WebError> {
    let Some(area_id) = area_id else {
        return Ok(crate::handlers::format::mentionable_users_json(Vec::new()));
    };
    let role =
        crate::handlers::access::project_role(state, &user.user_id, project_id, area_id).await?;
    let users = anamnesis_app::list_mentionable_users(
        state.membership.as_ref(),
        state.group_membership.as_ref(),
        state.user_directory.as_ref(),
        role,
        project_id,
        area_id,
    )
    .await?;
    Ok(crate::handlers::format::mentionable_users_json(users))
}
