//! A single project: its own flat task list (`docs/DOMAIN.md` §8's
//! "project-as-flat-list"), independent of the horizon each task sits at —
//! contrast with the global task board (`crate::handlers::board`), which
//! only ever shows tasks above it.

use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use minijinja::context;

use anamnesis_app::{AppError, create_task, view_project};
use anamnesis_core::ProjectId;

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::access;
use super::forms::CreateTaskForm;

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
    render_project_page(state, user, &aggregate, &tasks, None, StatusCode::OK)
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
            render_project_page(
                state,
                user,
                &aggregate,
                &tasks,
                Some(&e.to_string()),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
        }
        Err(err) => Err(WebError::from(err)),
    }
}

fn render_project_page(
    state: &AppState,
    user: &CurrentUser,
    aggregate: &anamnesis_app::ProjectAggregate,
    tasks: &[anamnesis_core::Task],
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, WebError> {
    let below: Vec<_> = tasks.iter().filter(|t| t.placement.is_below()).collect();
    let on_board: Vec<_> = tasks.iter().filter(|t| t.placement.is_on_board()).collect();

    let tmpl = state
        .templates
        .get_template("project.html")
        .map_err(WebError::template)?;
    let body = tmpl
        .render(context! {
            project => aggregate.project,
            below => below,
            on_board => on_board,
            csrf_token => user.csrf_token,
            current_user => user.display_name,
            error => error,
        })
        .map_err(WebError::template)?;
    Ok((status, Html(body)).into_response())
}
