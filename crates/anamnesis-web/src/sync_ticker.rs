//! Polls every project with sync enabled and reconciles it against its
//! configured external issue tracker (issues #40/#41) — the ticker half of
//! `anamnesis_app::use_cases::sync`; see that module's doc comment for the
//! reconciliation algorithm itself and why polling, not webhooks, is this
//! app's v1 approach (no public-ingress infrastructure exists here, and a
//! self-hosted deployment may have no reachable public URL at all).
//!
//! Deliberately simpler than `crate::sweep`'s ticker, in the same way
//! `crate::upload_gc`'s is: there is no user-facing calendar schedule to
//! honour, only a fixed poll interval. Different from both of those in one
//! respect: the lease is acquired **per project**, not once for the whole
//! tick, so one slow or failing project's sync can never block or delay
//! another's on the same instance.
//!
//! Like every other ticker in this crate, [`spawn_ticker`] is called from
//! exactly one place (`main.rs`), never from `routes::build_router`,
//! `AppState` construction, or `bootstrap::run` — so no integration test
//! (which builds a `Router` directly via `tests/support::TestApp`) can ever
//! cause it to spawn.

use std::time::Duration;

use anamnesis_app::ProjectSyncConfig;
use anamnesis_core::ProjectId;
use anamnesis_core::policy::Role;

use crate::handlers::project_sync::run_one_sync;
use crate::state::AppState;

/// How often the ticker checks every enabled project's sync config.
const POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Comfortably longer than one project's reconciliation pass should ever
/// take, short enough that a crashed instance's stale claim on one project
/// does not block that project's sync for long.
const SYNC_LEASE_TTL: Duration = Duration::from_secs(10 * 60);

fn lease_name(project_id: ProjectId) -> String {
    format!("project_sync:{project_id}")
}

/// Spawns the ticker, returning a handle `main.rs` aborts (never awaits) on
/// shutdown — the same fire-and-forget discipline `sweep::spawn_ticker`
/// documents: a reconciliation pass abandoned mid-run leaves nothing worse
/// than what it was already reconciling, and picks back up cleanly on the
/// next tick.
pub fn spawn_ticker(state: AppState) -> tokio::task::JoinHandle<()> {
    let owner = state.id_gen.next().to_string();
    tokio::spawn(async move {
        loop {
            tick_once(&state, &owner).await;
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
}

async fn tick_once(state: &AppState, owner: &str) {
    let configs = match state.project_sync_configs.list_enabled().await {
        Ok(configs) => configs,
        Err(err) => {
            tracing::error!(error = %err, "project sync: failed to list enabled configs");
            return;
        }
    };
    if configs.is_empty() {
        return;
    }
    // Nothing this tick can do without a cipher to decrypt a stored token
    // with — logged once per tick rather than once per config.
    if state.token_cipher.is_none() {
        tracing::warn!(
            project_count = configs.len(),
            "project sync: enabled, but no ANAMNESIS_SYNC_ENCRYPTION_KEY is configured"
        );
        return;
    }
    for config in configs {
        sync_one_project(state, owner, config).await;
    }
}

async fn sync_one_project(state: &AppState, owner: &str, config: ProjectSyncConfig) {
    let project_id = config.project_id;
    let lease = lease_name(project_id);
    match acquire_sync_lease(state, owner, &lease).await {
        Ok(true) => {}
        Ok(false) => return, // another instance is already syncing this project
        Err(err) => {
            tracing::error!(project_id = %project_id, error = %err, "project sync: failed to acquire lease");
            return;
        }
    }

    // System Admin: the ticker runs with no logged-in user, and the config
    // it acts on has already passed `Action::ManageProjectSync` once, at
    // the moment it was created or edited through the web UI — mirrors
    // `sweep::run_sweep`'s own `Some(Role::SystemAdmin)`, for the same
    // reason: a background job is not any particular user's request.
    match run_one_sync(state, Some(Role::SystemAdmin), &config).await {
        Ok(outcome) => tracing::info!(
            project_id = %project_id,
            pushed_new_task_count = outcome.pushed_new_task_count,
            imported_task_count = outcome.imported_task_count,
            pushed_to_remote_count = outcome.pushed_to_remote_count,
            pulled_from_remote_count = outcome.pulled_from_remote_count,
            imported_comment_count = outcome.imported_comment_count,
            "project sync: reconciled"
        ),
        Err(err) => {
            tracing::warn!(project_id = %project_id, error = ?err, "project sync: reconciliation failed");
        }
    }

    // Best-effort, same reasoning as `sweep::tick_once`'s own release: a
    // failed release just costs the rest of the TTL, not worth failing an
    // otherwise successful pass over.
    if let Err(err) = state.leases.release(&lease, owner).await {
        tracing::warn!(
            project_id = %project_id,
            error = %err,
            "project sync: could not release lease; it will expire on its own"
        );
    }
}

async fn acquire_sync_lease(
    state: &AppState,
    owner: &str,
    lease: &str,
) -> Result<bool, anamnesis_app::RepoError> {
    let now = state.clock.now();
    state.leases.try_acquire(lease, owner, now, SYNC_LEASE_TTL).await
}
