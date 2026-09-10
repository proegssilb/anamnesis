//! Configuring a project's external issue-tracker sync (issues #40/#41),
//! and triggering an immediate reconciliation pass — the project-page
//! analog of `crate::handlers::settings`, but Project Admin (or System
//! Admin) scoped rather than System Admin only, per
//! `anamnesis_app::policy::Action::ManageProjectSync`.

use axum::Form;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};

use anamnesis_app::{
    AppError, ProjectSyncConfig, SyncPorts, SyncProvider, configure_or_update_project_sync,
    run_and_record, view_sync_status,
};
use anamnesis_core::ProjectId;

use crate::auth::CurrentUser;
use crate::error::WebError;
use crate::session::csrf_tokens_match;
use crate::state::AppState;

use super::access;
use super::forms::{ConfigureSyncForm, CsrfOnlyForm};
use super::projects::render_project_page_reloaded;

fn bad(message: impl Into<String>) -> WebError {
    WebError::BadRequest(message.into())
}

fn parse_provider(raw: &str) -> Result<SyncProvider, WebError> {
    match raw {
        "github" => Ok(SyncProvider::GitHub),
        "forgejo" => Ok(SyncProvider::Forgejo),
        other => Err(bad(format!("{other:?} is not a recognised sync provider"))),
    }
}

fn non_empty(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Resolves the caller's role on `project_id` — the one preamble both
/// [`configure_sync_impl`] and [`trigger_sync_now_impl`] need before doing
/// anything else, pulled out so neither repeats the load-then-resolve
/// shape.
async fn project_role_for(
    state: &AppState,
    user: &CurrentUser,
    project_id: ProjectId,
) -> Result<Option<anamnesis_core::policy::Role>, WebError> {
    let project = state
        .projects
        .load(project_id)
        .await?
        .ok_or(AppError::NotFound)?;
    access::project_role(state, &user.user_id, project_id, project.project.area_id).await
}

/// Resolves the encrypted token bytes to store: a submitted token is
/// encrypted fresh, a blank one keeps whatever the project already has.
/// Returns [`AppError::SyncNotConfigured`] for a blank token on a project
/// with no existing config — there is nothing to keep.
async fn resolve_token(
    state: &AppState,
    role: Option<anamnesis_core::policy::Role>,
    project_id: ProjectId,
    raw: &str,
) -> Result<Vec<u8>, WebError> {
    let trimmed = raw.trim();
    let cipher = state
        .token_cipher
        .as_ref()
        .ok_or_else(|| bad("project sync is not enabled on this deployment (no encryption key configured)"))?;
    if !trimmed.is_empty() {
        return cipher.encrypt(trimmed).map_err(WebError::from);
    }
    let existing = view_sync_status(state.project_sync_configs.as_ref(), role, project_id).await?;
    let existing = existing.ok_or_else(|| bad("a personal access token is required"))?;
    Ok(existing.encrypted_token)
}

pub async fn configure_sync_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<ConfigureSyncForm>,
) -> Response {
    match configure_sync_impl(&state, &user, ProjectId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn configure_sync_impl(
    state: &AppState,
    user: &CurrentUser,
    project_id: ProjectId,
    form: ConfigureSyncForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let role = project_role_for(state, user, project_id).await?;

    let provider = parse_provider(&form.provider)?;
    let base_url = non_empty(&form.base_url);
    let encrypted_token = resolve_token(state, role, project_id, &form.token).await?;

    let result = configure_or_update_project_sync(
        state.project_sync_configs.as_ref(),
        state.clock.as_ref(),
        role,
        project_id,
        provider,
        base_url,
        &form.owner,
        &form.repo,
        encrypted_token,
        !form.auto_import_new_issues.is_empty(),
        !form.auto_push_new_tasks.is_empty(),
        !form.enabled.is_empty(),
    )
    .await;

    match result {
        Ok(_) => Ok(Redirect::to(&format!("/projects/{project_id}#project-settings")).into_response()),
        Err(AppError::Forbidden) => Err(WebError::App(AppError::Forbidden)),
        Err(AppError::Invalid(message)) => {
            render_project_page_reloaded(
                state,
                user,
                role,
                project_id,
                Some(&message),
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await
        }
        Err(other) => Err(WebError::from(other)),
    }
}

pub async fn trigger_sync_now_handler(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<uuid::Uuid>,
    Form(form): Form<CsrfOnlyForm>,
) -> Response {
    match trigger_sync_now_impl(&state, &user, ProjectId::new(id), form).await {
        Ok(response) => response,
        Err(err) => err.into_response_with(&state.templates),
    }
}

async fn trigger_sync_now_impl(
    state: &AppState,
    user: &CurrentUser,
    project_id: ProjectId,
    form: CsrfOnlyForm,
) -> Result<Response, WebError> {
    if !csrf_tokens_match(&user.csrf_token, &form.csrf_token) {
        return Err(WebError::CsrfMismatch);
    }
    let role = project_role_for(state, user, project_id).await?;

    let config = view_sync_status(state.project_sync_configs.as_ref(), role, project_id)
        .await?
        .ok_or(AppError::SyncNotConfigured)?;
    run_one_sync(state, role, &config).await?;
    Ok(Redirect::to(&format!("/projects/{project_id}#project-settings")).into_response())
}

/// Decrypts the config's token, builds the matching HTTP client, and runs
/// one reconciliation pass — the same function
/// `crate::sync_ticker::tick_once` calls for its own scheduled pass, so
/// "Sync now" and the background ticker are guaranteed to behave
/// identically.
pub(crate) async fn run_one_sync(
    state: &AppState,
    role: Option<anamnesis_core::policy::Role>,
    config: &ProjectSyncConfig,
) -> Result<anamnesis_app::SyncOutcome, WebError> {
    let cipher = state
        .token_cipher
        .as_ref()
        .ok_or_else(|| bad("project sync is not enabled on this deployment"))?;
    let token = cipher.decrypt(&config.encrypted_token).map_err(WebError::from)?;
    let client = anamnesis_adapters::build_client(config.provider, config.base_url.as_deref(), &token)
        .map_err(|e| WebError::from(AppError::from(e)))?;
    let ports = SyncPorts {
        configs: state.project_sync_configs.as_ref(),
        links: state.task_sync_links.as_ref(),
        tasks: state.tasks.as_ref(),
        comments: state.comments.as_ref(),
        board: state.board.as_ref(),
        search: state.search_index.as_ref(),
        clock: state.clock.as_ref(),
        ids: state.id_gen.as_ref(),
    };
    Ok(run_and_record(&ports, &client, role, config).await?)
}
