//! The area grid (`GET /areas`) and, for a single area, its **project
//! board**: `docs/DOMAIN.md` §3's fixed Pending/Active/Complete lanes,
//! derived from `Project.status` — a kanban of *projects*, not tasks. The
//! global *task* board lives at `crate::handlers::board`.

use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use minijinja::context;

use anamnesis_app::{
    AppError, create_area, create_project, edit_area, list_areas, list_projects_in_area,
    transition_project_status, view_area,
};
use anamnesis_core::policy::Role;
use anamnesis_core::{AreaId, Project, ProjectStatus};

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::access;
use super::forms::{CreateAreaForm, CreateProjectForm, EditAreaForm, TransitionProjectStatusForm};

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
    render_areas_page(state, user, &visible, admin, None, StatusCode::OK)
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
    render_area_page(
        state,
        user,
        &area,
        &projects,
        can_manage,
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
            render_area_page(
                state,
                user,
                &area,
                &projects,
                can_manage,
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
            render_area_page(
                state,
                user,
                &area,
                &projects,
                can_manage,
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
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<TransitionProjectStatusForm>,
) -> Response {
    match transition_project_status_impl(&state, &user, id, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn transition_project_status_impl(
    state: &AppState,
    user: &CurrentUser,
    project_id: uuid::Uuid,
    form: TransitionProjectStatusForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let project_id = anamnesis_core::ProjectId::new(project_id);
    let new_status = parse_status(&form.status)?;

    let aggregate = state
        .projects
        .load(project_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let area_id = aggregate.project.area_id;
    let role = access::project_role(state, &user.user_id, project_id, area_id).await?;
    // A live read, not a value cached at startup -- this is what makes
    // editing the limit through `/settings` actually change what gets
    // enforced on the very next request.
    let active_project_limit = state.settings.load().await?.active_project_limit;

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
        Ok(_) => Ok(Redirect::to(&format!("/areas/{area_id}")).into_response()),
        Err(AppError::ActiveProjectLimitExceeded) | Err(AppError::Rule(_)) => {
            let area = view_area(state.areas.as_ref(), Some(Role::Member), area_id).await?;
            let projects =
                list_projects_in_area(state.projects.as_ref(), Some(Role::Member), area_id).await?;
            let can_manage = matches!(role, Some(Role::SystemAdmin) | Some(Role::ProjectAdmin));
            render_area_page(
                state,
                user,
                &area,
                &projects,
                can_manage,
                Some("The active project limit has been reached."),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
        }
        Err(err) => Err(WebError::from(err)),
    }
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

fn render_areas_page(
    state: &AppState,
    user: &CurrentUser,
    areas: &[anamnesis_core::Area],
    can_manage: bool,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, WebError> {
    let tmpl = state
        .templates
        .get_template("areas.html")
        .map_err(WebError::template)?;
    let body = tmpl
        .render(context! {
            areas => areas,
            can_manage => can_manage,
            csrf_token => user.csrf_token,
            current_user => user.display_name,
            error => error,
        })
        .map_err(WebError::template)?;
    Ok((status, Html(body)).into_response())
}

fn render_area_page(
    state: &AppState,
    user: &CurrentUser,
    area: &anamnesis_core::Area,
    projects: &[Project],
    can_manage: bool,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, WebError> {
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

    let tmpl = state
        .templates
        .get_template("area.html")
        .map_err(WebError::template)?;
    let body = tmpl
        .render(context! {
            area => area,
            pending => pending,
            active => active,
            complete => complete,
            can_manage => can_manage,
            csrf_token => user.csrf_token,
            current_user => user.display_name,
            error => error,
        })
        .map_err(WebError::template)?;
    Ok((status, Html(body)).into_response())
}
