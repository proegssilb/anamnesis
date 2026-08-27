//! Global search across areas, projects, and tasks (`docs/DOMAIN.md` §8),
//! via `crate::ports`... no — via `anamnesis_app`'s [`anamnesis_app::SearchQuery`]
//! port. Open to any authenticated user, exactly like `crate::handlers::board`
//! (see that module's doc comment): a search result names nothing beyond
//! what its own page already reveals to whoever can reach it, and every
//! result link still resolves through that page's own real permission check.

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use minijinja::context;
use serde::Deserialize;

use anamnesis_app::SearchHit;

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::hx::is_hx_request;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    #[serde(default)]
    pub q: String,
}

pub async fn search_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
    Query(params): Query<SearchParams>,
) -> Response {
    match search_impl(&state, &user, &headers, params.q).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn search_impl(
    state: &AppState,
    user: &CurrentUser,
    headers: &HeaderMap,
    query: String,
) -> Result<Response, WebError> {
    let trimmed = query.trim().to_string();
    let hits = if trimmed.is_empty() {
        Vec::new()
    } else {
        state.search.search(&trimmed).await?
    };
    let results: Vec<_> = hits.iter().map(hit_view).collect();

    // `docs/DOMAIN.md` §8: "one endpoint, two representations" — a fragment
    // for htmx (a live-search `hx-trigger="keyup changed delay:300ms"` on
    // the search box), a full page for everything else, including the
    // no-JS plain `<form method="get">` fallback.
    if is_hx_request(headers) {
        let tmpl = state
            .templates
            .get_template("_search_results.html")
            .map_err(WebError::template)?;
        let body = tmpl
            .render(context! { query => trimmed, results => results })
            .map_err(WebError::template)?;
        return Ok(Html(body).into_response());
    }

    let tmpl = state
        .templates
        .get_template("search.html")
        .map_err(WebError::template)?;
    let body = tmpl
        .render(context! {
            query => trimmed,
            results => results,
            csrf_token => user.csrf_token,
            current_user => user.display_name,
        })
        .map_err(WebError::template)?;
    Ok(Html(body).into_response())
}

fn hit_view(hit: &SearchHit) -> minijinja::Value {
    match hit {
        SearchHit::Area { id, title } => context! {
            kind => "area",
            title => title.as_str(),
            url => format!("/areas/{id}"),
        },
        SearchHit::Project { id, title } => context! {
            kind => "project",
            title => title.as_str(),
            url => format!("/projects/{id}"),
        },
        SearchHit::Task { id, title } => context! {
            kind => "task",
            title => title.as_str(),
            url => format!("/tasks/{id}"),
        },
    }
}
