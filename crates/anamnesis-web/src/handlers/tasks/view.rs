use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use minijinja::context;
use serde::Deserialize;

use anamnesis_app::view_task;
use anamnesis_core::TaskId;

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::state::AppState;

use super::page::render_task_page;
use super::role_for_task;

#[derive(Debug, Deserialize)]
pub struct ViewTaskParams {
    #[serde(default)]
    pub rel_to: Option<String>,
}

pub async fn view_task_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Query(params): Query<ViewTaskParams>,
) -> Response {
    match view_task_impl(&state, &user, TaskId::new(id), params.rel_to).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn view_task_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    rel_to: Option<String>,
) -> Result<Response, WebError> {
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    let aggregate = view_task(state.tasks.as_ref(), role, task_id).await?;

    // Lands here after a "Select" click in the relationship-candidate search
    // (`_relationship_candidates.html`): the id is just a UI hint to prefill
    // `#task-add-relationship`'s `to_task_id`, not itself a permission
    // decision, so an unparseable or now-missing id degrades to "no prefill"
    // rather than failing the whole page load — the same tolerance the
    // relationship-other-end title lookup a little further down already
    // extends to a stale id.
    let relationship_prefill = resolve_rel_to_context(state, rel_to.as_deref()).await?;

    render_task_page(
        state,
        user,
        task_id,
        &aggregate.task,
        None,
        None,
        relationship_prefill,
        StatusCode::OK,
    )
    .await
}

/// Resolves an incoming `?rel_to=<id>` into the context the relationship
/// modal needs to show it as already selected — `None` for a missing param,
/// an unparseable id, or an id that no longer names a task, all treated
/// alike as "no prefill" rather than an error (see [`view_task_impl`]'s doc
/// comment on this same tolerance).
async fn resolve_rel_to_context(
    state: &AppState,
    rel_to: Option<&str>,
) -> Result<Option<minijinja::Value>, WebError> {
    let Some(Ok(raw)) = rel_to.map(uuid::Uuid::parse_str) else {
        return Ok(None);
    };
    let candidate_id = TaskId::new(raw);
    Ok(state
        .tasks
        .load(candidate_id)
        .await?
        .map(|a| context! { id => candidate_id.to_string(), title => a.task.title.as_str() }))
}
