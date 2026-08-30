//! The title-search engine shared by the parent-task picker
//! ([`super::hierarchy`]) and the relationship-target picker
//! ([`super::relationships`]): both are a search over
//! `anamnesis_app::SearchQuery`, scoped down to `SearchHit::Task` and with
//! the current task's own id dropped from the results, differing only in
//! which pair of templates (an htmx fragment, and the standalone no-JS page)
//! they render the candidates into.

use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use minijinja::context;

use anamnesis_app::{SearchHit, view_task};
use anamnesis_core::TaskId;

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::hx::is_hx_request;
use crate::state::AppState;

use super::role_for_task;

/// The search half of [`task_candidates_impl`]: trims the query, and (when
/// non-empty) runs it through `anamnesis_app::SearchQuery`, scoped down to
/// `SearchHit::Task` and with this task's own id dropped from the results.
/// Split out from the rendering so each half reads as one job.
async fn search_task_candidates(
    state: &AppState,
    task_id: TaskId,
    query: String,
) -> Result<(String, Vec<minijinja::Value>), WebError> {
    let trimmed = query.trim().to_string();
    let mut candidates: Vec<minijinja::Value> = Vec::new();
    if !trimmed.is_empty() {
        let hits = state.search.search(&trimmed).await?;
        for hit in &hits {
            if let SearchHit::Task { id: hit_id, title } = hit
                && *hit_id != task_id
            {
                candidates.push(context! { id => hit_id.to_string(), title => title.as_str() });
            }
        }
    }
    Ok((trimmed, candidates))
}

/// The rendering half of [`task_candidates_impl`]: given the already-computed
/// search results, picks between the htmx fragment and the standalone no-JS
/// page per `docs/DOMAIN.md` §8's "one endpoint, two representations".
fn render_task_candidates(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    task_title: &str,
    headers: &HeaderMap,
    search_results: (String, Vec<minijinja::Value>),
    templates: (&str, &str),
) -> Result<Response, WebError> {
    let (trimmed, candidates) = search_results;
    let (fragment_template, page_template) = templates;
    let (template_name, ctx) = if is_hx_request(headers) {
        (
            fragment_template,
            context! {
                task_id => task_id.to_string(),
                query => trimmed,
                candidates => candidates,
                csrf_token => user.csrf_token,
            },
        )
    } else {
        (
            page_template,
            context! {
                task_id => task_id.to_string(),
                task_title => task_title,
                query => trimmed,
                candidates => candidates,
                csrf_token => user.csrf_token,
                current_user => user.display_name,
            },
        )
    };
    let tmpl = state
        .templates
        .get_template(template_name)
        .map_err(WebError::template)?;
    let body = tmpl.render(ctx).map_err(WebError::template)?;
    Ok(Html(body).into_response())
}

/// The shared body of the parent and relationship candidate handlers: both
/// are a title search over `anamnesis_app::SearchQuery`, scoped down to
/// `SearchHit::Task` and with this task's own id dropped from the results,
/// differing only in which pair of templates (an htmx fragment, and the
/// standalone no-JS page) they render the candidates into. A thin
/// orchestrator over [`search_task_candidates`] and [`render_task_candidates`].
pub(super) async fn task_candidates_impl(
    state: &AppState,
    user: &CurrentUser,
    task_id: TaskId,
    headers: &HeaderMap,
    query: String,
    fragment_template: &str,
    page_template: &str,
) -> Result<Response, WebError> {
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    let aggregate = view_task(state.tasks.as_ref(), role, task_id).await?;

    let search_results = search_task_candidates(state, task_id, query).await?;

    render_task_candidates(
        state,
        user,
        task_id,
        aggregate.task.title.as_str(),
        headers,
        search_results,
        (fragment_template, page_template),
    )
}
