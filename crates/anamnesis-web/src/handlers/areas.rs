//! The area grid (`GET /areas`) and, for a single area, its **project
//! board**: `docs/DOMAIN.md` §3's fixed Pending/Active/Complete lanes,
//! derived from `Project.status` — a kanban of *projects*, not tasks. The
//! global *task* board lives at `crate::handlers::board`.

use axum::Form;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use minijinja::context;

use anamnesis_app::{
    AppError, create_area, create_project, edit_area, list_areas, list_projects_in_area,
    transition_project_status, view_area,
};
use anamnesis_core::policy::Role;
use anamnesis_core::{AreaId, Project, ProjectId, ProjectStatus};

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::hx::is_hx_request;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::access;
use super::forms::{CreateAreaForm, CreateProjectForm, EditAreaForm, TransitionProjectStatusForm};
use super::group_membership::{self, AccessPanel};

/// Rebuilds the area page from scratch — the four reads `render_area_page`
/// needs — for the error paths that must re-render it after a failed
/// mutation. Mirrors `crate::handlers::projects::render_project_page_reloaded`.
///
/// `view_area`/`list_projects_in_area` are read at a fixed `Some(Role::Member)`
/// rather than `role`, matching the one pre-existing call site
/// (`transition_project_status_impl`'s error arm) this helper replaces —
/// preserved as-is rather than changed as a drive-by.
async fn render_area_page_reloaded(
    state: &AppState,
    user: &CurrentUser,
    role: Option<Role>,
    area_id: AreaId,
    can_manage: bool,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, WebError> {
    let area = view_area(state.areas.as_ref(), Some(Role::Member), area_id).await?;
    let projects =
        list_projects_in_area(state.projects.as_ref(), Some(Role::Member), area_id).await?;
    let access_panel = group_membership::area_panel(state, role, area_id, can_manage).await?;
    render_area_page(state, user, &area, &projects, &access_panel, error, status)
}

pub async fn list_areas_handler(State(state): State<AppState>, user: CurrentUser) -> Response {
    match list_areas_impl(&state, &user).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn list_areas_impl(state: &AppState, user: &CurrentUser) -> Result<Response, WebError> {
    // `list_areas`'s permission gate is a single global `Option<Role>` — it
    // has no per-area scoping of its own to check a listing that spans many
    // areas against. Resolved here as: any authenticated caller may run the
    // listing at all (any `Some(_)` role clears `is_allowed`'s gate), and
    // the *actual* per-area visibility (`docs/DOMAIN.md` §3's membership
    // model) is enforced below by filtering to areas the caller holds an
    // effective role on — exactly what `view_area` itself already checks
    // one area at a time.
    let areas = list_areas(state.areas.as_ref(), Some(Role::Member)).await?;
    let admin = access::is_system_admin(state, &user.user_id).await?;
    let mut visible = Vec::with_capacity(areas.len());
    for area in areas {
        if admin
            || access::area_role(state, &user.user_id, area.id)
                .await?
                .is_some()
        {
            visible.push(area);
        }
    }
    render_areas_page(state, user, &visible, admin, None, StatusCode::OK).await
}

pub async fn create_area_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Form(form): Form<CreateAreaForm>,
) -> Response {
    match create_area_impl(&state, &user, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn create_area_impl(
    state: &AppState,
    user: &CurrentUser,
    form: CreateAreaForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    // A brand-new area has no scope of its own to resolve a role in
    // (`crate::use_cases::area`'s doc comment): only a true System Admin can
    // ever satisfy `Action::ManageArea` here.
    let admin = access::is_system_admin(state, &user.user_id).await?;
    let role = admin.then_some(Role::SystemAdmin);
    let position = state.areas.list().await?.len() as u32;
    match create_area(
        state.areas.as_ref(),
        state.id_gen.as_ref(),
        state.clock.as_ref(),
        state.search_index.as_ref(),
        role,
        &form.title,
        &form.description,
        position,
    )
    .await
    {
        Ok(area) => {
            // Indexing for global search (`docs/DOMAIN.md` §8) now happens
            // inside `create_area` itself, alongside the repository write —
            // this handler is transport only. See
            // `anamnesis_app::use_cases::indexing`'s module doc comment for
            // why that boundary matters (any non-web caller of the use case
            // must get a consistent index too) and what happens if the
            // index write itself fails.
            Ok(Redirect::to(&format!("/areas/{}", area.id)).into_response())
        }
        Err(AppError::Rule(e)) => {
            let areas = state.areas.list().await?;
            render_areas_page(
                state,
                user,
                &areas,
                admin,
                Some(&e.to_string()),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await
        }
        Err(err) => Err(WebError::from(err)),
    }
}

pub async fn view_area_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    match view_area_impl(&state, &user, AreaId::new(id)).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn view_area_impl(
    state: &AppState,
    user: &CurrentUser,
    area_id: AreaId,
) -> Result<Response, WebError> {
    let role = access::area_role(state, &user.user_id, area_id).await?;
    let area = view_area(state.areas.as_ref(), role, area_id).await?;
    let projects = list_projects_in_area(state.projects.as_ref(), role, area_id).await?;
    let can_manage = matches!(role, Some(Role::SystemAdmin) | Some(Role::ProjectAdmin));
    let access_panel = group_membership::area_panel(state, role, area_id, can_manage).await?;
    render_area_page(
        state,
        user,
        &area,
        &projects,
        &access_panel,
        None,
        StatusCode::OK,
    )
}

pub async fn edit_area_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<EditAreaForm>,
) -> Response {
    match edit_area_impl(&state, &user, AreaId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn edit_area_impl(
    state: &AppState,
    user: &CurrentUser,
    area_id: AreaId,
    form: EditAreaForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let role = access::area_role(state, &user.user_id, area_id).await?;
    match edit_area(
        state.areas.as_ref(),
        state.clock.as_ref(),
        state.search_index.as_ref(),
        role,
        area_id,
        &form.title,
        &form.description,
    )
    .await
    {
        Ok(_) => {
            // Re-indexed inside `edit_area` itself — see
            // `create_area_impl`'s comment above.
            Ok(Redirect::to(&format!("/areas/{area_id}")).into_response())
        }
        Err(AppError::Rule(e)) => {
            let area = view_area(state.areas.as_ref(), role, area_id).await?;
            let projects = list_projects_in_area(state.projects.as_ref(), role, area_id).await?;
            let can_manage = matches!(role, Some(Role::SystemAdmin) | Some(Role::ProjectAdmin));
            let access_panel =
                group_membership::area_panel(state, role, area_id, can_manage).await?;
            render_area_page(
                state,
                user,
                &area,
                &projects,
                &access_panel,
                Some(&e.to_string()),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
        }
        Err(err) => Err(WebError::from(err)),
    }
}

pub async fn create_project_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<CreateProjectForm>,
) -> Response {
    match create_project_impl(&state, &user, AreaId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn create_project_impl(
    state: &AppState,
    user: &CurrentUser,
    area_id: AreaId,
    form: CreateProjectForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let role = access::area_role(state, &user.user_id, area_id).await?;
    match create_project(
        state.projects.as_ref(),
        state.id_gen.as_ref(),
        state.clock.as_ref(),
        state.search_index.as_ref(),
        role,
        area_id,
        &form.title,
        &form.description,
    )
    .await
    {
        Ok(project) => {
            // Indexed inside `create_project` itself — see
            // `create_area_impl`'s comment above.
            Ok(Redirect::to(&format!("/projects/{}", project.id)).into_response())
        }
        Err(AppError::Rule(e)) => {
            let area = view_area(state.areas.as_ref(), role, area_id).await?;
            let projects = list_projects_in_area(state.projects.as_ref(), role, area_id).await?;
            let can_manage = matches!(role, Some(Role::SystemAdmin) | Some(Role::ProjectAdmin));
            let access_panel =
                group_membership::area_panel(state, role, area_id, can_manage).await?;
            render_area_page(
                state,
                user,
                &area,
                &projects,
                &access_panel,
                Some(&e.to_string()),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
        }
        Err(err) => Err(WebError::from(err)),
    }
}

pub async fn transition_project_status_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<TransitionProjectStatusForm>,
) -> Response {
    match transition_project_status_impl(&state, &user, &headers, id, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

/// The outcome of attempting a project's status transition, independent of
/// how it will end up being rendered — that's `respond_to_transition`'s job.
enum TransitionOutcome {
    Ok,
    ActiveProjectLimitExceeded,
    Rule(String),
}

/// Attempts the transition itself and classifies the result. A rule
/// violation or a hit against the active-project limit is business-as-usual
/// here (the caller decides how to show it); anything else is a genuine
/// [`WebError`].
async fn apply_status_transition(
    state: &AppState,
    role: Option<Role>,
    project_id: ProjectId,
    new_status: ProjectStatus,
    active_project_limit: u32,
) -> Result<TransitionOutcome, WebError> {
    match transition_project_status(
        state.projects.as_ref(),
        state.clock.as_ref(),
        role,
        project_id,
        new_status,
        active_project_limit,
    )
    .await
    {
        Ok(_) => Ok(TransitionOutcome::Ok),
        Err(AppError::ActiveProjectLimitExceeded) => {
            Ok(TransitionOutcome::ActiveProjectLimitExceeded)
        }
        Err(AppError::Rule(e)) => Ok(TransitionOutcome::Rule(e.to_string())),
        Err(err) => Err(WebError::from(err)),
    }
}

/// Turns a [`TransitionOutcome`] into a response, in whichever of the two
/// shapes this request wants:
///
/// - An htmx drag always gets the re-rendered lanes back, success or not —
///   there is no per-card spot to surface an error mid-drag, exactly like
///   `crate::handlers::projects::raise_project_task_impl`'s
///   `WipLimitExceeded` branch on the same shape of problem. On failure the
///   lanes just reflect the true (unchanged) DB state.
/// - A plain form post gets a redirect on success, or the whole area page
///   reloaded with the error on failure.
async fn respond_to_transition(
    state: &AppState,
    user: &CurrentUser,
    headers: &HeaderMap,
    role: Option<Role>,
    area_id: AreaId,
    can_manage: bool,
    outcome: TransitionOutcome,
) -> Result<Response, WebError> {
    if is_hx_request(headers) {
        return render_area_lanes_fragment(state, area_id, can_manage, &user.csrf_token).await;
    }
    let message = match outcome {
        TransitionOutcome::Ok => {
            return Ok(Redirect::to(&format!("/areas/{area_id}")).into_response());
        }
        TransitionOutcome::ActiveProjectLimitExceeded => {
            "The active project limit has been reached.".to_string()
        }
        TransitionOutcome::Rule(message) => message,
    };
    render_area_page_reloaded(
        state,
        user,
        role,
        area_id,
        can_manage,
        Some(&message),
        StatusCode::UNPROCESSABLE_ENTITY,
    )
    .await
}

async fn transition_project_status_impl(
    state: &AppState,
    user: &CurrentUser,
    headers: &HeaderMap,
    project_id: uuid::Uuid,
    form: TransitionProjectStatusForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let project_id = ProjectId::new(project_id);
    let new_status = parse_status(&form.status)?;

    let aggregate = state
        .projects
        .load(project_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let area_id = aggregate.project.area_id;
    let role = access::project_role(state, &user.user_id, project_id, area_id).await?;
    let can_manage = matches!(role, Some(Role::SystemAdmin) | Some(Role::ProjectAdmin));
    // A live read, not a value cached at startup -- this is what makes
    // editing the limit through `/settings` actually change what gets
    // enforced on the very next request.
    let active_project_limit = state.settings.load().await?.active_project_limit;

    let outcome =
        apply_status_transition(state, role, project_id, new_status, active_project_limit).await?;
    respond_to_transition(state, user, headers, role, area_id, can_manage, outcome).await
}

fn parse_status(raw: &str) -> Result<ProjectStatus, WebError> {
    match raw {
        "pending" => Ok(ProjectStatus::Pending),
        "active" => Ok(ProjectStatus::Active),
        "complete" => Ok(ProjectStatus::Complete),
        other => Err(WebError::BadRequest(format!(
            "{other:?} is not a known project status"
        ))),
    }
}

async fn render_areas_page(
    state: &AppState,
    user: &CurrentUser,
    areas: &[anamnesis_core::Area],
    can_manage: bool,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, WebError> {
    // The "N projects" count on each card (`docs/DOMAIN.md` §3's area grid)
    // — non-archived projects only, the same count `list_projects_in_area`
    // already gives the area's own board.
    let mut rows = Vec::with_capacity(areas.len());
    for area in areas {
        let count =
            list_projects_in_area(state.projects.as_ref(), Some(Role::Member), area.id).await?;
        rows.push(context! {
            id => area.id.to_string(),
            title => area.title.as_str(),
            description => area.description.as_str(),
            project_count => count.len(),
        });
    }

    let tmpl = state
        .templates
        .get_template("areas.html")
        .map_err(WebError::template)?;
    let body = tmpl
        .render(context! {
            areas => rows,
            can_manage => can_manage,
            // `can_manage` here is always exactly "is this caller a System
            // Admin" -- creating an area has nowhere else to hang a role
            // (see this function's only two call sites) -- so it doubles as
            // the nav link's admin gate with no extra membership lookup.
            is_system_admin => can_manage,
            csrf_token => user.csrf_token,
            current_user => user.display_name,
            error => error,
        })
        .map_err(WebError::template)?;
    Ok((status, Html(body)).into_response())
}

/// Splits `projects` into the area page's three Pending/Active/Complete
/// lanes, each shaped for `_area_project_list.html`
/// (`title`/`list_id`/`role`/`projects`), fragment-addressable like
/// `crate::handlers::projects`'s `_project_task_list.html`
/// (`docs/DOMAIN.md` §8). Shared by `render_area_page` (the full page) and
/// `render_area_lanes_fragment` (the status-transition htmx response) so
/// both render identical markup from the same source of truth.
fn build_area_sections(projects: &[Project]) -> Vec<minijinja::Value> {
    fn lane(title: &str, list_id: &str, role: &str, items: &[&Project]) -> minijinja::Value {
        let views: Vec<_> = items
            .iter()
            .map(|p| {
                context! {
                    id => p.id.to_string(),
                    title => p.title.as_str(),
                    description => p.description.as_str(),
                }
            })
            .collect();
        context! {
            title => title,
            list_id => list_id,
            role => role,
            projects => views,
        }
    }

    let pending: Vec<_> = projects
        .iter()
        .filter(|p| p.status == ProjectStatus::Pending)
        .collect();
    let active: Vec<_> = projects
        .iter()
        .filter(|p| p.status == ProjectStatus::Active)
        .collect();
    let complete: Vec<_> = projects
        .iter()
        .filter(|p| p.status == ProjectStatus::Complete)
        .collect();
    vec![
        lane("Pending", "pending-list", "pending", &pending),
        lane("Active", "active-list", "active", &active),
        lane("Complete", "complete-list", "complete", &complete),
    ]
}

/// The `HX-Request` response for changing a project's status by dragging it
/// between lanes on the area page: all three `_area_project_list.html`
/// lanes, each marked `hx-swap-oob="true"` — the
/// `crate::handlers::projects::render_project_lists_fragment` pattern,
/// since a status change always potentially touches two lanes (the project
/// leaves one, joins the other).
async fn render_area_lanes_fragment(
    state: &AppState,
    area_id: AreaId,
    can_manage: bool,
    csrf_token: &str,
) -> Result<Response, WebError> {
    let projects =
        list_projects_in_area(state.projects.as_ref(), Some(Role::Member), area_id).await?;
    let sections = build_area_sections(&projects);
    let contexts = sections.into_iter().map(|section| {
        context! {
            section => section,
            area_id => area_id.to_string(),
            can_manage => can_manage,
            csrf_token => csrf_token,
            oob => true,
        }
    });
    super::render_oob_fragments(&state.templates, "_area_project_list.html", contexts)
}

fn render_area_page(
    state: &AppState,
    user: &CurrentUser,
    area: &anamnesis_core::Area,
    projects: &[Project],
    panel: &AccessPanel,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, WebError> {
    let board_sections = build_area_sections(projects);
    let (members, groups) = panel.member_and_group_context();

    let tmpl = state
        .templates
        .get_template("area.html")
        .map_err(WebError::template)?;
    let body = tmpl
        .render(context! {
            area => area,
            area_id => area.id.to_string(),
            board_sections => board_sections,
            members => members,
            groups => groups,
            known_groups => panel.known_groups,
            show_groups => panel.show_groups(),
            can_manage => panel.can_manage,
            csrf_token => user.csrf_token,
            current_user => user.display_name,
            error => error,
        })
        .map_err(WebError::template)?;
    Ok((status, Html(body)).into_response())
}
