//! A single project: its own flat task list (`docs/DOMAIN.md` §8's
//! "project-as-flat-list"), independent of the horizon each task sits at —
//! contrast with the global task board (`crate::handlers::board`), which
//! only ever shows tasks above it.

use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use minijinja::context;
use serde::Deserialize;

use anamnesis_app::{
    AppError, archive_project, create_task, list_all_projects, list_project_members,
    unarchive_project, view_project,
};
use anamnesis_core::UserId;
use anamnesis_core::policy::Role;
use anamnesis_core::{Project, ProjectId, ProjectStatus};

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::access;
use super::field_form;
use super::format::format_field_kind;
use super::forms::{AddFieldDefinitionForm, CreateTaskForm, CsrfOnlyForm};
use super::membership::format_role;

/// The Project's "members" section, fetched only when `can_manage` — the
/// [`crate::handlers::areas::area_members_for_display`] sibling.
async fn project_members_for_display(
    state: &AppState,
    role: Option<Role>,
    project_id: ProjectId,
    can_manage: bool,
) -> Result<Vec<(UserId, Role)>, WebError> {
    if !can_manage {
        return Ok(Vec::new());
    }
    Ok(list_project_members(state.membership.as_ref(), role, project_id).await?)
}

pub async fn view_project_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    match view_project_impl(&state, &user, ProjectId::new(id)).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn view_project_impl(
    state: &AppState,
    user: &CurrentUser,
    project_id: ProjectId,
) -> Result<Response, WebError> {
    let aggregate = state
        .projects
        .load(project_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let area_id = aggregate.project.area_id;
    let role = access::project_role(state, &user.user_id, project_id, area_id).await?;
    let aggregate = view_project(state.projects.as_ref(), role, project_id).await?;
    let tasks = state.tasks.list_by_project(project_id).await?;
    let can_manage = matches!(role, Some(Role::SystemAdmin) | Some(Role::ProjectAdmin));
    let members = project_members_for_display(state, role, project_id, can_manage).await?;
    render_project_page(
        state,
        user,
        &aggregate,
        &tasks,
        &members,
        can_manage,
        None,
        StatusCode::OK,
    )
}

pub async fn create_task_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<CreateTaskForm>,
) -> Response {
    match create_task_impl(&state, &user, ProjectId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn create_task_impl(
    state: &AppState,
    user: &CurrentUser,
    project_id: ProjectId,
    form: CreateTaskForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let aggregate = state
        .projects
        .load(project_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let area_id = aggregate.project.area_id;
    let role = access::project_role(state, &user.user_id, project_id, area_id).await?;

    match create_task(
        state.tasks.as_ref(),
        state.id_gen.as_ref(),
        state.clock.as_ref(),
        state.search_index.as_ref(),
        role,
        project_id,
        &form.title,
        &form.description,
    )
    .await
    {
        Ok(task) => {
            // Indexed inside `create_task` itself — see
            // `anamnesis_app::use_cases::indexing`'s module doc comment.
            Ok(Redirect::to(&format!("/tasks/{}", task.id)).into_response())
        }
        Err(AppError::Rule(e)) => {
            let aggregate = view_project(state.projects.as_ref(), role, project_id).await?;
            let tasks = state.tasks.list_by_project(project_id).await?;
            let can_manage = matches!(role, Some(Role::SystemAdmin) | Some(Role::ProjectAdmin));
            let members = project_members_for_display(state, role, project_id, can_manage).await?;
            render_project_page(
                state,
                user,
                &aggregate,
                &tasks,
                &members,
                can_manage,
                Some(&e.to_string()),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
        }
        Err(err) => Err(WebError::from(err)),
    }
}

/// Archives a project (`docs/DOMAIN.md` §2), gated on Project Admin (or
/// System Admin) — `anamnesis_app::policy::Action::ArchiveProject`.
pub async fn archive_project_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<CsrfOnlyForm>,
) -> Response {
    match archive_project_impl(&state, &user, ProjectId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn archive_project_impl(
    state: &AppState,
    user: &CurrentUser,
    project_id: ProjectId,
    form: CsrfOnlyForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let aggregate = state
        .projects
        .load(project_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let area_id = aggregate.project.area_id;
    let role = access::project_role(state, &user.user_id, project_id, area_id).await?;
    archive_project(
        state.projects.as_ref(),
        state.clock.as_ref(),
        state.search_index.as_ref(),
        role,
        project_id,
    )
    .await?;
    Ok(Redirect::to(&format!("/projects/{project_id}")).into_response())
}

/// Restores an archived project.
pub async fn unarchive_project_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<CsrfOnlyForm>,
) -> Response {
    match unarchive_project_impl(&state, &user, ProjectId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn unarchive_project_impl(
    state: &AppState,
    user: &CurrentUser,
    project_id: ProjectId,
    form: CsrfOnlyForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let aggregate = state
        .projects
        .load(project_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let area_id = aggregate.project.area_id;
    let role = access::project_role(state, &user.user_id, project_id, area_id).await?;
    unarchive_project(
        state.projects.as_ref(),
        state.clock.as_ref(),
        state.search_index.as_ref(),
        role,
        project_id,
    )
    .await?;
    Ok(Redirect::to(&format!("/projects/{project_id}")).into_response())
}

/// Defines a new custom field on a project (`docs/DOMAIN.md` §3) — the
/// house-hunting motivating example (price, viewing date, ...), previously
/// only reachable by hand-writing SQL. Gated on Project Admin (or System
/// Admin) via `anamnesis_app::policy::Action::ManageFieldDefinitions`: field
/// vocabulary is structural, project-admin work, not ordinary task work.
pub async fn add_field_definition_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<AddFieldDefinitionForm>,
) -> Response {
    match add_field_definition_impl(&state, &user, ProjectId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn add_field_definition_impl(
    state: &AppState,
    user: &CurrentUser,
    project_id: ProjectId,
    form: AddFieldDefinitionForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let aggregate = state
        .projects
        .load(project_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let area_id = aggregate.project.area_id;
    let role = access::project_role(state, &user.user_id, project_id, area_id).await?;
    let kind = field_form::parse_field_kind(&form.kind)?;
    // Every existing field definition's `position` is already occupied — the
    // new field lands after all of them, exactly like `create_area_impl`
    // sizes a new area's grid position from the current list length.
    let position = aggregate.field_definitions.len() as u32;

    match anamnesis_app::add_field_definition(
        state.projects.as_ref(),
        state.id_gen.as_ref(),
        role,
        project_id,
        &form.name,
        kind,
        position,
        !form.show_on_card.is_empty(),
    )
    .await
    {
        Ok(_) => Ok(Redirect::to(&format!("/projects/{project_id}")).into_response()),
        Err(AppError::Rule(e)) => {
            let aggregate = view_project(state.projects.as_ref(), role, project_id).await?;
            let tasks = state.tasks.list_by_project(project_id).await?;
            let can_manage = matches!(role, Some(Role::SystemAdmin) | Some(Role::ProjectAdmin));
            let members = project_members_for_display(state, role, project_id, can_manage).await?;
            render_project_page(
                state,
                user,
                &aggregate,
                &tasks,
                &members,
                can_manage,
                Some(&e.to_string()),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
        }
        Err(err) => Err(WebError::from(err)),
    }
}

#[allow(clippy::too_many_arguments)]
fn render_project_page(
    state: &AppState,
    user: &CurrentUser,
    aggregate: &anamnesis_app::ProjectAggregate,
    tasks: &[anamnesis_core::Task],
    members: &[(UserId, Role)],
    can_manage: bool,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, WebError> {
    let below: Vec<_> = tasks.iter().filter(|t| t.placement.is_below()).collect();
    let on_board: Vec<_> = tasks.iter().filter(|t| t.placement.is_on_board()).collect();

    // This project's own custom field vocabulary (`docs/DOMAIN.md` §3) — the
    // house-hunting example's price/viewing-date/... definitions, so a
    // project admin can see what already exists before adding another one.
    let fields: Vec<_> = aggregate
        .field_definitions
        .iter()
        .map(|def| {
            context! {
                id => def.id.to_string(),
                name => def.name.as_str(),
                kind => format_field_kind(def.kind),
                show_on_card => def.show_on_card,
            }
        })
        .collect();
    let members: Vec<_> = members
        .iter()
        .map(|(user, role)| context! { user_id => user.to_string(), role => format_role(*role) })
        .collect();

    let tmpl = state
        .templates
        .get_template("project.html")
        .map_err(WebError::template)?;
    let body = tmpl
        .render(context! {
            project => aggregate.project,
            below => below,
            on_board => on_board,
            fields => fields,
            members => members,
            can_manage => can_manage,
            csrf_token => user.csrf_token,
            current_user => user.display_name,
            error => error,
        })
        .map_err(WebError::template)?;
    Ok((status, Html(body)).into_response())
}

/// Filter/sort state for the system-wide Projects data grid (`GET
/// /projects`). Every field is a plain `String` with `#[serde(default)]` —
/// the same shape `crate::handlers::search::SearchParams` uses — so a bare
/// `GET /projects` with no query string at all lands on the intended
/// defaults (`status`/`archived` empty means "non-archived, non-completed",
/// per this page's own spec) rather than a 400.
#[derive(Debug, Deserialize)]
pub struct ProjectListParams {
    /// `""` (default: Pending + Active only) | `"pending"` | `"active"` |
    /// `"complete"` | `"all"`.
    #[serde(default)]
    pub status: String,
    /// An [`anamnesis_core::AreaId`]'s text form, or `""` for every area.
    #[serde(default)]
    pub area: String,
    /// The "include archived" checkbox — present only when checked, exactly
    /// like `SearchParams::archived`.
    #[serde(default)]
    pub archived: String,
    /// `"title"` (default) | `"area"` | `"status"` | `"created"` | `"updated"`.
    #[serde(default)]
    pub sort: String,
    /// `"asc"` (default) | `"desc"`.
    #[serde(default)]
    pub dir: String,
}

pub async fn list_projects_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(params): Query<ProjectListParams>,
) -> Response {
    match list_projects_impl(&state, &user, params).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn list_projects_impl(
    state: &AppState,
    user: &CurrentUser,
    params: ProjectListParams,
) -> Result<Response, WebError> {
    let admin = access::is_system_admin(state, &user.user_id).await?;

    // Every area this caller may see at all — the same per-area membership
    // filter `crate::handlers::areas::list_areas_impl` applies to the area
    // grid, reused here so a project never leaks through a listing that has
    // no area-level scoping of its own (see `list_all_projects`'s doc
    // comment).
    let all_areas = state.areas.list().await?;
    let mut visible_areas = Vec::with_capacity(all_areas.len());
    for area in all_areas {
        if admin
            || access::area_role(state, &user.user_id, area.id)
                .await?
                .is_some()
        {
            visible_areas.push(area);
        }
    }
    visible_areas.sort_by(|a, b| a.title.as_str().cmp(b.title.as_str()));

    let area_filter = uuid::Uuid::parse_str(params.area.trim())
        .ok()
        .map(anamnesis_core::AreaId::new);

    let all_projects = list_all_projects(state.projects.as_ref(), Some(Role::Member)).await?;
    let area_titles: std::collections::HashMap<_, _> = visible_areas
        .iter()
        .map(|a| (a.id, a.title.as_str().to_string()))
        .collect();

    let include_archived = !params.archived.is_empty();
    let status_filter = params.status.as_str();

    let mut projects: Vec<Project> = all_projects
        .into_iter()
        .filter(|p| area_titles.contains_key(&p.area_id))
        .filter(|p| include_archived || p.archived_at.is_none())
        .filter(|p| match status_filter {
            "all" => true,
            "pending" => p.status == ProjectStatus::Pending,
            "active" => p.status == ProjectStatus::Active,
            "complete" => p.status == ProjectStatus::Complete,
            // Default: hide Complete projects until asked for explicitly.
            _ => p.status != ProjectStatus::Complete,
        })
        .filter(|p| area_filter.is_none_or(|area_id| p.area_id == area_id))
        .collect();

    let sort = match params.sort.as_str() {
        "area" | "status" | "created" | "updated" => params.sort.as_str(),
        _ => "title",
    };
    let dir_desc = params.dir == "desc";
    projects.sort_by(|a, b| {
        let ordering = match sort {
            "area" => area_titles
                .get(&a.area_id)
                .cmp(&area_titles.get(&b.area_id)),
            "status" => format_status(a.status).cmp(format_status(b.status)),
            "created" => a
                .created_at
                .unix_seconds()
                .cmp(&b.created_at.unix_seconds()),
            "updated" => a
                .updated_at
                .unix_seconds()
                .cmp(&b.updated_at.unix_seconds()),
            _ => a.title.as_str().cmp(b.title.as_str()),
        };
        if dir_desc {
            ordering.reverse()
        } else {
            ordering
        }
    });

    let rows: Vec<_> = projects
        .iter()
        .map(|p| {
            context! {
                id => p.id.to_string(),
                title => p.title.as_str(),
                status => format_status(p.status),
                area_id => p.area_id.to_string(),
                area_title => area_titles.get(&p.area_id).cloned().unwrap_or_default(),
                archived => p.archived_at.is_some(),
                created_at => p.created_at.unix_seconds(),
                updated_at => p.updated_at.unix_seconds(),
            }
        })
        .collect();

    let area_options: Vec<_> = visible_areas
        .iter()
        .map(|a| context! { id => a.id.to_string(), title => a.title.as_str() })
        .collect();

    let dir_for = |field: &str| {
        if sort == field && !dir_desc {
            "desc"
        } else {
            "asc"
        }
    };
    let sort_link = |field: &str| {
        format!(
            "/projects?status={status}&area={area}&archived={archived}&sort={field}&dir={dir}",
            status = status_filter,
            area = params.area,
            archived = if include_archived { "1" } else { "" },
            field = field,
            dir = dir_for(field),
        )
    };

    let tmpl = state
        .templates
        .get_template("projects.html")
        .map_err(WebError::template)?;
    let body = tmpl
        .render(context! {
            projects => rows,
            areas => area_options,
            status => status_filter,
            area => params.area,
            include_archived => include_archived,
            sort => sort,
            dir => if dir_desc { "desc" } else { "asc" },
            sort_link_title => sort_link("title"),
            sort_link_area => sort_link("area"),
            sort_link_status => sort_link("status"),
            sort_link_created => sort_link("created"),
            sort_link_updated => sort_link("updated"),
            csrf_token => user.csrf_token,
            current_user => user.display_name,
            is_system_admin => admin,
        })
        .map_err(WebError::template)?;
    Ok(Html(body).into_response())
}

fn format_status(status: ProjectStatus) -> &'static str {
    match status {
        ProjectStatus::Pending => "Pending",
        ProjectStatus::Active => "Active",
        ProjectStatus::Complete => "Complete",
    }
}
