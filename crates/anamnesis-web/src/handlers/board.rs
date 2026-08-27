//! The global task board (`docs/DOMAIN.md` §2, §3): every column, the tasks
//! and placed tangles currently above the horizon in each, and the
//! suggestion prompt — `docs/DOMAIN.md` §5, "the soul of the product". Get
//! `Outcome::Full`'s silence exactly right: no banner, no nudge, nothing
//! rendered at all.
//!
//! **Scope decision.** The board spans every active project and area at
//! once (`docs/DOMAIN.md` §3), so — unlike an Area or a Project — it has no
//! single scope `crate::handlers::access` can resolve a role against, and
//! `anamnesis_app::policy::Action` defines no board-level action to check.
//! Rather than invent an `Action::ViewBoard` this phase's design doc does
//! not ask for, viewing the aggregate board and requesting a suggestion are
//! open to any authenticated user; every individual task shown was already
//! placed there by someone who held a real per-project role when they raised
//! it, and every mutating action here (`raise_task`/`place_tangle` behind
//! accepting a suggestion, `drop_tangle`) still resolves and enforces a real
//! per-project role.
//!
//! **Resolving a role for a tangle action.** A `Tangle` spans however many
//! tasks (and, potentially, projects) make up its knot — there is no single
//! "the tangle's project" the way there is for a task. Placing/dropping one
//! is authorized against the project of its *lowest task id* (`BTreeSet`
//! already gives a deterministic first element) — an arbitrary but stable
//! and documented choice, not a silent one; see [`role_for_tangle`].

use std::collections::HashMap;

use axum::Form;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use minijinja::context;

use anamnesis_app::{
    AppError, BoardItem, BoardItemKind, archive_done_tasks, drop_tangle, place_tangle, raise_task,
    reposition_board_item, request_suggestion, resolve_frozen_tangles, run_tangle_detection,
};
use anamnesis_core::policy::Role;
use anamnesis_core::{Blockage, ColumnId, OfferItem, Outcome, ProjectId, Tangle, TangleId, TaskId};

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::hx::is_hx_request;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::format::format_field_data;
use super::forms::{AcceptSuggestionForm, AcceptTangleForm, CsrfOnlyForm, RepositionForm};
use super::tasks::role_for_task;

pub async fn view_board_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
) -> Response {
    match view_board_impl(&state, &user, &headers, None, StatusCode::OK).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

/// No `HX-Request` header exists on these mutation handlers' own incoming
/// request in the same way a page navigation carries one — they redirect on
/// success, and only fall through to re-rendering the board (with an error
/// banner) when the underlying use case rejects the request. That
/// re-render always wants the full page: none of these three actions are
/// reachable from an htmx-driven partial request in this phase's UI.
fn full_page_headers() -> HeaderMap {
    HeaderMap::new()
}

/// Moves a board card (a task or a placed tangle) — the htmx drag endpoint
/// and its plain-form fallback both post here (`docs/DOMAIN.md` §8:
/// "Sortable drags, htmx persists"; see `static/app.js` and
/// `templates/_reposition_form.html`).
///
/// **One endpoint, two representations.** An `HX-Request` gets back just
/// the column(s) the move actually touched, each as an out-of-band fragment
/// (`templates/_column.html`'s `oob` flag) so htmx can swap them in place;
/// a plain form submit gets the usual redirect-after-POST, since a no-JS
/// browser is about to reload the page anyway.
pub async fn reposition_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
    Form(form): Form<RepositionForm>,
) -> Response {
    match reposition_impl(&state, &user, &headers, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn reposition_impl(
    state: &AppState,
    user: &CurrentUser,
    headers: &HeaderMap,
    form: RepositionForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let item = match form.item_kind.as_str() {
        "task" => BoardItemKind::Task(TaskId::new(form.item_id)),
        "tangle" => BoardItemKind::Tangle(TangleId::new(form.item_id)),
        other => {
            return Err(WebError::BadRequest(format!(
                "{other:?} is not a known board item kind"
            )));
        }
    };
    let role = match item {
        BoardItemKind::Task(id) => role_for_task(state, &user.user_id, id).await?.1,
        BoardItemKind::Tangle(id) => role_for_tangle(state, &user.user_id, id).await?,
    };
    let column = ColumnId::new(form.column_id);

    // The source column (if any), read *before* the move, so its fragment
    // can be re-rendered too when the card changed columns — after the
    // move, this is no longer discoverable from the item itself.
    let previous_column = current_column_of(state, item).await?;

    match reposition_board_item(
        state.tasks.as_ref(),
        state.tangles.as_ref(),
        state.board.as_ref(),
        state.clock.as_ref(),
        role,
        item,
        column,
        form.position,
    )
    .await
    {
        Ok(()) => {}
        Err(AppError::WipLimitExceeded) => {
            return view_board_impl(
                state,
                user,
                headers,
                Some("That column is already at its work-in-progress limit."),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await;
        }
        Err(err) => return Err(WebError::from(err)),
    }

    if is_hx_request(headers) {
        return render_reposition_fragment(state, user, column, previous_column).await;
    }
    Ok(Redirect::to("/board").into_response())
}

/// `item`'s current column, if it is on the board at all — `None` for a
/// task/tangle below the horizon, which `reposition_board_item` (being
/// asked to raise it) treats as having no "old column" to renumber.
async fn current_column_of(
    state: &AppState,
    item: BoardItemKind,
) -> Result<Option<ColumnId>, WebError> {
    use anamnesis_core::Placement;
    let placement = match item {
        BoardItemKind::Task(id) => {
            state
                .tasks
                .load(id)
                .await?
                .ok_or(AppError::NotFound)?
                .task
                .placement
        }
        BoardItemKind::Tangle(id) => {
            state
                .tangles
                .load(id)
                .await?
                .ok_or(AppError::NotFound)?
                .placement
        }
    };
    Ok(match placement {
        Placement::OnBoard { column, .. } => Some(column),
        Placement::Below => None,
    })
}

/// Renders one or two `_column.html` fragments, each marked
/// `hx-swap-oob="true"`, for the columns a reposition actually touched —
/// just `column` when the card stayed put, both `column` and
/// `previous_column` when it crossed columns.
async fn render_reposition_fragment(
    state: &AppState,
    user: &CurrentUser,
    column: ColumnId,
    previous_column: Option<ColumnId>,
) -> Result<Response, WebError> {
    let columns = state.board.columns_with_items().await?;
    let column_views = build_column_views(state, &columns).await?;

    let mut targets = vec![column];
    if let Some(prev) = previous_column
        && prev != column
    {
        targets.push(prev);
    }

    let tmpl = state
        .templates
        .get_template("_column.html")
        .map_err(WebError::template)?;
    let mut body = String::new();
    for target in targets {
        let Some(idx) = columns.iter().position(|bc| bc.column.id == target) else {
            continue;
        };
        body.push_str(
            &tmpl
                .render(context! {
                    c => column_views[idx].clone(),
                    columns => column_views,
                    csrf_token => user.csrf_token,
                    oob => true,
                })
                .map_err(WebError::template)?,
        );
    }
    Ok(Html(body).into_response())
}

/// Resolves the role to authorize a placement action on `tangle_id` against
/// — see the module doc comment for why "the project of its lowest task id"
/// is the (documented) rule.
pub(super) async fn role_for_tangle(
    state: &AppState,
    user_id: &anamnesis_core::UserId,
    tangle_id: TangleId,
) -> Result<Option<Role>, WebError> {
    let tangle = state
        .tangles
        .load(tangle_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let anchor_task = *tangle
        .task_ids
        .iter()
        .next()
        .expect("a Tangle's task_ids is never empty by construction");
    let (_, role) = role_for_task(state, user_id, anchor_task).await?;
    Ok(role)
}

async fn view_board_impl(
    state: &AppState,
    user: &CurrentUser,
    headers: &HeaderMap,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, WebError> {
    // Keeps tangle state current before it is read below (the "quiet
    // indicator", and the suggestion engine's tangled-task exclusion) —
    // detection is a pure reconciliation pass, cheap to re-run per view at
    // this phase's scale; a scheduled job is a natural later home for it.
    // Frozen (placed) tangles are untouched by detection — see
    // `run_tangle_detection`'s own doc comment — so `resolve_frozen_tangles`
    // is the separate pass that closes one out once its frozen task set is
    // no longer cyclic in the live graph.
    run_tangle_detection(
        state.relationships.as_ref(),
        state.tangles.as_ref(),
        state.id_gen.as_ref(),
        state.clock.as_ref(),
    )
    .await?;

    let done_column = state
        .board
        .columns_with_items()
        .await?
        .into_iter()
        .find(|bc| bc.column.is_done)
        .map(|bc| bc.column.id);
    resolve_frozen_tangles(
        state.relationships.as_ref(),
        state.tangles.as_ref(),
        state.board.as_ref(),
        state.clock.as_ref(),
        done_column,
    )
    .await?;
    // Re-read after resolution: a tangle that just closed may have moved
    // into the `is_done` column, and the view must reflect that.
    let columns = state.board.columns_with_items().await?;

    let active_tangles = state.tangles.list_active().await?;

    // The entry column — where a suggestion, once accepted, lands — is the
    // board's first (lowest-position) column: `crate::bootstrap` always
    // seeds To-Do at position 0, the one column `docs/DOMAIN.md` §3 actually
    // calls out as WIP-limited.
    let suggestion = match columns.first() {
        Some(entry) => Some(fetch_suggestion(state, user, entry.column.id).await?),
        None => None,
    };

    // `docs/DOMAIN.md` §8: "one endpoint, two representations" — an
    // `HX-Request` gets just the columns fragment (useful for an
    // htmx-driven refresh that never wants the surrounding page again),
    // everything else gets the full page.
    if is_hx_request(headers) {
        let column_views = build_column_views(state, &columns).await?;
        let tmpl = state
            .templates
            .get_template("_board_columns.html")
            .map_err(WebError::template)?;
        let body = tmpl
            .render(context! { columns => column_views, csrf_token => user.csrf_token })
            .map_err(WebError::template)?;
        return Ok((status, Html(body)).into_response());
    }

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
    let columns = state.board.columns_with_items().await?;
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
                &full_page_headers(),
                Some("The entry column filled up before that suggestion could be accepted."),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await
        }
        Err(err) => Err(WebError::from(err)),
    }
}

/// Accepts a tangle offer from the suggestion prompt: places it on the
/// board's entry column (`docs/DOMAIN.md`'s Tangle section — "accepting the
/// offer places it").
pub async fn accept_tangle_offer_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Form(form): Form<AcceptTangleForm>,
) -> Response {
    match accept_tangle_offer_impl(&state, &user, form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn accept_tangle_offer_impl(
    state: &AppState,
    user: &CurrentUser,
    form: AcceptTangleForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let tangle_id = TangleId::new(form.tangle_id);
    let role = role_for_tangle(state, &user.user_id, tangle_id).await?;
    let columns = state.board.columns_with_items().await?;
    let entry = columns
        .first()
        .ok_or_else(|| WebError::BadRequest("no board columns are configured".to_string()))?;

    match place_tangle(
        state.tangles.as_ref(),
        state.board.as_ref(),
        role,
        tangle_id,
        entry.column.id,
    )
    .await
    {
        Ok(_) => Ok(Redirect::to("/board").into_response()),
        Err(AppError::WipLimitExceeded) => {
            view_board_impl(
                state,
                user,
                &full_page_headers(),
                Some("The entry column filled up before that tangle could be accepted."),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await
        }
        Err(err) => Err(WebError::from(err)),
    }
}

/// Drops a placed tangle back below the horizon — the board-card equivalent
/// of `crate::handlers::tasks::drop_task_handler` for a tangle.
pub async fn drop_tangle_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<CsrfOnlyForm>,
) -> Response {
    match drop_tangle_impl(&state, &user, TangleId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn drop_tangle_impl(
    state: &AppState,
    user: &CurrentUser,
    tangle_id: TangleId,
    form: CsrfOnlyForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let role = role_for_tangle(state, &user.user_id, tangle_id).await?;
    drop_tangle(state.tangles.as_ref(), role, tangle_id).await?;
    Ok(Redirect::to("/board").into_response())
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
        state.search_index.as_ref(),
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

/// Builds the per-column template context `board.html` (the full page) and
/// `render_reposition_fragment` (the htmx reposition response) both render
/// from — factored out so a reposition's fragment response reflects the
/// exact same card content (including `show_on_card` fields) as a full
/// board load, not a stripped-down copy of it.
async fn build_column_views(
    state: &AppState,
    columns: &[anamnesis_app::BoardColumn],
) -> Result<Vec<minijinja::Value>, WebError> {
    let mut field_def_cache: HashMap<ProjectId, Vec<anamnesis_core::FieldDefinition>> =
        HashMap::new();
    let mut column_views = Vec::with_capacity(columns.len());
    for bc in columns {
        // Tasks and placed tangles interleaved by position, in one list —
        // the same order `BoardQuery::columns_with_items` returns
        // (`docs/DOMAIN.md`'s Tangle section: one shared ordering, not
        // "tasks then tangles").
        let mut item_views = Vec::with_capacity(bc.items.len());
        for item in &bc.items {
            match item {
                BoardItem::Task(task) => {
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
                    item_views.push(context! {
                        kind => "task",
                        id => task.id.to_string(),
                        title => task.title.as_str(),
                        bounce_count => task.bounce_count,
                        card_fields => card_fields,
                    });
                }
                BoardItem::Tangle(tangle) => {
                    item_views.push(context! {
                        kind => "tangle",
                        id => tangle.id.to_string(),
                        size => tangle.task_ids.len(),
                        task_ids => tangle.task_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
                        resolved => tangle.resolved_at.is_some(),
                    });
                }
            }
        }
        // A placed tangle counts against the column's WIP limit exactly
        // like a task (`docs/DOMAIN.md`'s Tangle section) -- `items.len()`
        // already reflects that, since both kinds share this one list.
        column_views.push(context! {
            id => bc.column.id.to_string(),
            title => bc.column.title.as_str(),
            wip_limit => bc.column.wip_limit,
            current_count => bc.items.len(),
            is_done => bc.column.is_done,
            items => item_views,
        });
    }
    Ok(column_views)
}

async fn render_board_page(
    state: &AppState,
    user: &CurrentUser,
    columns: &[anamnesis_app::BoardColumn],
    active_tangles: &[Tangle],
    suggestion: Option<&Outcome>,
    error: Option<&str>,
    status: StatusCode,
) -> Result<Response, WebError> {
    let column_views = build_column_views(state, columns).await?;

    let tangle_views: Vec<_> = active_tangles
        .iter()
        .map(|t| {
            context! {
                id => t.id.to_string(),
                size => t.task_ids.len(),
                task_ids => t.task_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
                on_board => t.placement.is_on_board(),
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
