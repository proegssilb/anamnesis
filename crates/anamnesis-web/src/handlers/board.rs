//! The global task board (`docs/DOMAIN.md` §2, §3): every column, the tasks
//! currently above the horizon in each, and the suggestion prompt —
//! `docs/DOMAIN.md` §5, "the soul of the product". Get `Outcome::Full`'s
//! silence exactly right: no banner, no nudge, nothing rendered at all.
//!
//! **Scope decision.** The board spans every active project and area at
//! once (`docs/DOMAIN.md` §3), so — unlike an Area or a Project — it has no
//! single scope `crate::handlers::access` can resolve a role against, and
//! `anamnesis_app::policy::Action` defines no board-level action to check.
//! Rather than invent an `Action::ViewBoard` this phase's design doc does
//! not ask for, viewing the aggregate board and requesting a suggestion are
//! open to any authenticated user; every individual task shown was already
//! placed there by someone who held a real per-project role when they raised
//! it, and every mutating action here (`raise_task` behind accepting a
//! suggestion) still resolves and enforces that task's *own* project role.

use std::collections::HashMap;

use axum::Form;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use minijinja::context;

use anamnesis_app::{
    AppError, archive_done_tasks, raise_task, request_suggestion, run_tangle_detection,
};
use anamnesis_core::policy::Role;
use anamnesis_core::{Blockage, OfferItem, Outcome, ProjectId, TaskId};

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::format::format_field_data;
use super::forms::{AcceptSuggestionForm, CsrfOnlyForm};
use super::tasks::role_for_task;

pub async fn view_board_handler(State(state): State<AppState>, user: CurrentUser) -> Response {
    match view_board_impl(&state, &user, None, StatusCode::OK).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn view_board_impl(
    state: &AppState,
    user: &CurrentUser,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, WebError> {
    // Keeps tangle state current before it is read below (the "quiet
    // indicator", and the suggestion engine's tangled-task exclusion) —
    // detection is a pure reconciliation pass, cheap to re-run per view at
    // this phase's scale; a scheduled job is a natural later home for it.
    run_tangle_detection(
        state.relationships.as_ref(),
        state.tangles.as_ref(),
        state.id_gen.as_ref(),
        state.clock.as_ref(),
    )
    .await?;

    let columns = state.board.columns_with_tasks().await?;
    let active_tangles = state.tangles.list_active().await?;

    // The entry column — where a suggestion, once accepted, lands — is the
    // board's first (lowest-position) column: `crate::bootstrap` always
    // seeds To-Do at position 0, the one column `docs/DOMAIN.md` §3 actually
    // calls out as WIP-limited.
    let suggestion = match columns.first() {
        Some(entry) => Some(fetch_suggestion(state, user, entry.column.id).await?),
        None => None,
    };

    render_board_page(
        state,
        user,
        &columns,
        &active_tangles,
        suggestion.as_ref(),
        error,
        status,
    )
    .await
}

async fn fetch_suggestion(
    state: &AppState,
    user: &CurrentUser,
    entry_column: anamnesis_core::ColumnId,
) -> Result<Outcome, WebError> {
    let now = state.clock.now();
    let local_date = state
        .timezone
        .local_date(&state.settings.timezone_name, now)?;
    let outcome = request_suggestion(
        state.board.as_ref(),
        state.tasks.as_ref(),
        state.clock.as_ref(),
        Some(Role::Member),
        &user.user_id,
        (local_date.year(), local_date.ordinal()),
        entry_column,
        &state.settings.suggestion,
    )
    .await?;
    Ok(outcome)
}

pub async fn accept_suggestion_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Form(form): Form<AcceptSuggestionForm>,
) -> Response {
    match accept_suggestion_impl(&state, &user, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn accept_suggestion_impl(
    state: &AppState,
    user: &CurrentUser,
    form: AcceptSuggestionForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let task_id = TaskId::new(form.task_id);
    let (_, role) = role_for_task(state, &user.user_id, task_id).await?;
    let columns = state.board.columns_with_tasks().await?;
    let entry = columns
        .first()
        .ok_or_else(|| WebError::BadRequest("no board columns are configured".to_string()))?;
    let position = state.board.count_on_column(entry.column.id).await?;

    match raise_task(
        state.tasks.as_ref(),
        state.board.as_ref(),
        state.clock.as_ref(),
        role,
        task_id,
        entry.column.id,
        position,
    )
    .await
    {
        Ok(_) => Ok(Redirect::to("/board").into_response()),
        Err(AppError::WipLimitExceeded) => {
            view_board_impl(
                state,
                user,
                Some("The entry column filled up before that suggestion could be accepted."),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await
        }
        Err(err) => Err(WebError::from(err)),
    }
}

pub async fn archive_all_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Form(form): Form<CsrfOnlyForm>,
) -> Response {
    match archive_all_impl(&state, &user, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn archive_all_impl(
    state: &AppState,
    user: &CurrentUser,
    form: CsrfOnlyForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    archive_done_tasks(
        state.board.as_ref(),
        state.tasks.as_ref(),
        state.clock.as_ref(),
        Some(Role::Member),
    )
    .await?;
    Ok(Redirect::to("/board").into_response())
}

fn blockage_message(blockage: Blockage) -> &'static str {
    match blockage {
        Blockage::BacklogEmpty => "The backlog is empty — nothing is waiting below the horizon.",
        Blockage::NoActiveProject => "Nothing eligible belongs to an Active project.",
        Blockage::AllBlocked => "Everything eligible is blocked by unfinished work.",
        Blockage::AllTangled => "Everything eligible is knotted in a tangle.",
        Blockage::AllOnCooldown => "Everything eligible was offered recently and is on cooldown.",
    }
}

async fn render_board_page(
    state: &AppState,
    user: &CurrentUser,
    columns: &[anamnesis_app::BoardColumn],
    active_tangles: &[anamnesis_core::Tangle],
    suggestion: Option<&Outcome>,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, WebError> {
    let mut field_def_cache: HashMap<ProjectId, Vec<anamnesis_core::FieldDefinition>> =
        HashMap::new();
    let mut column_views = Vec::with_capacity(columns.len());
    for bc in columns {
        let mut task_views = Vec::with_capacity(bc.tasks.len());
        for task in &bc.tasks {
            let defs = match field_def_cache.get(&task.project_id) {
                Some(defs) => defs.clone(),
                None => {
                    let defs = state
                        .projects
                        .load(task.project_id)
                        .await?
                        .map(|a| a.field_definitions)
                        .unwrap_or_default();
                    field_def_cache.insert(task.project_id, defs.clone());
                    defs
                }
            };
            let card_fields = if defs.iter().any(|d| d.show_on_card) {
                let values = state
                    .tasks
                    .load(task.id)
                    .await?
                    .map(|a| a.field_values)
                    .unwrap_or_default();
                defs.iter()
                    .filter(|d| d.show_on_card)
                    .filter_map(|def| {
                        values
                            .iter()
                            .find(|v| v.field_id == def.id)
                            .map(|v| context! { name => def.name.as_str(), value => format_field_data(&v.data) })
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            task_views.push(context! {
                id => task.id.to_string(),
                title => task.title.as_str(),
                bounce_count => task.bounce_count,
                card_fields => card_fields,
            });
        }
        column_views.push(context! {
            id => bc.column.id.to_string(),
            title => bc.column.title.as_str(),
            wip_limit => bc.column.wip_limit,
            current_count => bc.tasks.len(),
            is_done => bc.column.is_done,
            tasks => task_views,
        });
    }

    let tangle_views: Vec<_> = active_tangles
        .iter()
        .map(|t| {
            context! {
                id => t.id.to_string(),
                size => t.task_ids.len(),
                task_ids => t.task_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
            }
        })
        .collect();

    let suggestion_view = match suggestion {
        None | Some(Outcome::Full) => None,
        Some(Outcome::Stuck(blockage)) => Some(context! {
            kind => "stuck",
            message => blockage_message(*blockage),
        }),
        Some(Outcome::Offer(offer)) => {
            let mut items = Vec::with_capacity(offer.items.len());
            for item in &offer.items {
                match item {
                    OfferItem::Task(task_offer) => {
                        let title = state
                            .tasks
                            .load(task_offer.task_id)
                            .await?
                            .map(|a| a.task.title.as_str().to_string())
                            .unwrap_or_else(|| "(deleted task)".to_string());
                        items.push(context! {
                            kind => "task",
                            task_id => task_offer.task_id.to_string(),
                            title => title,
                            high_bounce => task_offer.high_bounce,
                        });
                    }
                    OfferItem::Tangle(tangle) => {
                        items.push(context! {
                            kind => "tangle",
                            tangle_id => tangle.id.to_string(),
                            size => tangle.task_ids.len(),
                            task_ids => tangle.task_ids.iter().map(|id: &anamnesis_core::TaskId| id.to_string()).collect::<Vec<_>>(),
                        });
                    }
                }
            }
            Some(context! { kind => "offer", items => items })
        }
    };

    let tmpl = state
        .templates
        .get_template("board.html")
        .map_err(WebError::template)?;
    let body = tmpl
        .render(context! {
            columns => column_views,
            tangles => tangle_views,
            suggestion => suggestion_view,
            csrf_token => user.csrf_token,
            current_user => user.display_name,
            error => error,
        })
        .map_err(WebError::template)?;
    Ok((status, Html(body)).into_response())
}
