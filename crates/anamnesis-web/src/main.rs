#![forbid(unsafe_code)]
//! The binary entry point: resolve configuration, wire real adapters,
//! bootstrap a fresh database, build the router, and serve it. All the
//! actual logic lives in the library (`src/lib.rs` and its modules) so
//! integration tests can build the same `Router` in-process without a
//! socket.
//!
//! [`main`] is deliberately just the startup sequence: each stage below is a
//! named function, so the order of operations is readable in one screen and
//! every failure exits through [`fail`] rather than repeating an
//! `eprintln!` + `std::process::exit` pair.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use tracing_subscriber::EnvFilter;

use anamnesis_adapters::{
    FsBlobStore, OidcIdentityProvider, S3BlobStore, S3Settings, SqlStore, SystemClock,
    TzTimezoneResolver, UuidIdGen,
};
use anamnesis_app::{BlobStore, Clock, IdentityProvider, JobLease, TimezoneResolver};
use anamnesis_web::config::Config;
use anamnesis_web::state::AppState;
use anamnesis_web::{bootstrap, health, routes, session, sweep, tangles, templates};

#[tokio::main]
async fn main() {
    init_tracing();

    // Before anything else: the container image's HEALTHCHECK re-runs this
    // same binary (`distroless/cc` has neither a shell nor `curl`), and a
    // health probe must not need the full configuration a real startup does.
    if std::env::args().nth(1).as_deref() == Some("--health-check") {
        run_health_check().await;
    }

    let config =
        Config::from_env().unwrap_or_else(|err| fail(format!("configuration error: {err}")));

    if config.dev_auth_bypass {
        tracing::warn!(
            "ANAMNESIS_DEV_AUTH_BYPASS is enabled: every request is authenticated as a fixed \
             local user with no identity provider involved. This must never be set in a real \
             deployment."
        );
    }

    validate_timezone(&config.timezone);
    let store = open_store(&config).await;
    let leases = open_job_lease(&store).await;
    let identity = resolve_identity(&config).await;
    let blobs = open_blob_store(&config).await;
    let state = build_state(&config, store, blobs, identity, leases);

    // The two background tickers: the archive sweep (`docs/DOMAIN.md` §6) and
    // the tangle-detection backstop. Both are deliberately started only here,
    // in the binary -- never from `routes::build_router`, `AppState`
    // construction, or `bootstrap::run` -- so no integration test (which
    // builds a `Router` directly via `routes::build_router`, per
    // `tests/support`) can ever cause one to spawn. See each module's doc
    // comment for the full reasoning. They take the one `JobLease` store out
    // of `state` and coordinate on distinct job names, so neither can block
    // the other -- or the relationship handlers, which now hold that same
    // lease around their own detection passes.
    let sweep_handle = sweep::spawn_ticker(state.clone());
    let tangle_handle = tangles::spawn_backstop(state.clone());

    serve(routes::build_router(state), config.bind_addr).await;

    // Both tickers are detached background tasks with nothing left to flush.
    // A sweep either committed or it didn't, and `sweep_done` is idempotent,
    // so an abort mid-sweep is safe to resume on the next boot (see `sweep`'s
    // module doc comment); a detection pass recomputes its whole answer from
    // the graph on the next run, so an abort mid-pass leaves nothing partial
    // behind either. `abort()` returns immediately rather than waiting for
    // the next wake-up, so neither delays process exit.
    sweep_handle.abort();
    tangle_handle.abort();
}

/// Probes an already-running server and exits 0 (healthy) or 1 (not),
/// which is the whole contract a container `HEALTHCHECK` cares about.
///
/// Reads only `ANAMNESIS_BIND_ADDR`: demanding a database URL and an OIDC
/// client secret before the probe could run would make the health check fail
/// for reasons that have nothing to do with the server's health.
async fn run_health_check() -> ! {
    let addr = Config::bind_addr_from_env()
        .unwrap_or_else(|err| fail(format!("configuration error: {err}")));
    if health::check_health(addr).await {
        std::process::exit(0)
    }
    tracing::error!(addr = %addr, "health check failed");
    std::process::exit(1)
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
}

/// Reports a fatal startup problem and exits non-zero.
///
/// Goes through `tracing::error!` rather than `eprintln!` so startup failures
/// land in the same stream, with the same formatting and filtering, as every
/// other diagnostic the process emits — [`init_tracing`] runs first precisely
/// so this is true even for a configuration error.
fn fail(message: impl std::fmt::Display) -> ! {
    tracing::error!("{message}");
    std::process::exit(1)
}

/// Fails loudly, naming the variable, if `ANAMNESIS_TIMEZONE` does not name a
/// real IANA zone -- checked once, at startup, against "now", rather than
/// discovered lazily the first time a sweep or a suggestion needs it.
fn validate_timezone(name: &str) {
    if let Err(err) = TzTimezoneResolver::new().local_date(name, SystemClock.now()) {
        fail(format!("invalid ANAMNESIS_TIMEZONE {name:?}: {err}"));
    }
}

/// The database, connected and ready to serve: schema migrated by
/// `SqlStore::connect`, then bootstrapped (System Admin granted, default
/// board columns seeded) if this is a fresh one.
async fn open_store(config: &Config) -> Arc<SqlStore> {
    let store = SqlStore::connect(&config.database_url)
        .await
        .unwrap_or_else(|err| fail(format!("failed to connect to the database: {err}")));
    bootstrap::run(
        &store,
        &UuidIdGen,
        &config.bootstrap_admin,
        config.oidc_admin_group.as_deref(),
        &config.timezone,
    )
    .await
    .unwrap_or_else(|err| fail(format!("failed to bootstrap the database: {err}")));
    Arc::new(store)
}

/// The lease store every coordinated job runs against. On SQLite this is a
/// second file beside the data one, not another table in it — `SqlStore`'s
/// `lease_database_url` says why.
///
/// One store, shared by three callers that agree only on distinct job names:
/// the sweep ticker, the tangle-detection backstop, and — through
/// [`AppState::leases`] — the relationship handlers that drive detection on
/// the event path.
async fn open_job_lease(store: &SqlStore) -> Arc<dyn JobLease> {
    let leases = store
        .job_lease()
        .await
        .unwrap_or_else(|err| fail(format!("failed to open the job-lease store: {err}")));
    Arc::new(leases)
}

/// Discovers the configured OIDC provider, or `None` when there is nothing to
/// discover.
///
/// Dev bypass never needs a real provider — it skips discovery entirely even
/// if `ANAMNESIS_OIDC_*` happens to also be set (`config.rs` only makes those
/// optional in bypass mode, it does not forbid setting them), so a stray or
/// unreachable issuer URL can never break a `cargo run` with bypass on.
async fn resolve_identity(config: &Config) -> Option<Arc<dyn IdentityProvider>> {
    let issuer_url = match (&config.oidc_issuer_url, config.dev_auth_bypass) {
        (Some(issuer_url), false) => issuer_url,
        _ => return None,
    };
    let client_id = config
        .oidc_client_id
        .clone()
        .expect("ANAMNESIS_OIDC_CLIENT_ID is required whenever an issuer URL is set");
    let provider = OidcIdentityProvider::discover(
        issuer_url,
        client_id,
        config
            .oidc_client_secret
            .as_ref()
            .map(|secret| secret.expose().to_string()),
        format!("{}/auth/callback", config.base_url.trim_end_matches('/')),
        config.oidc_scopes.clone(),
        config.tls_ca_bundle.as_deref(),
        config.oidc_user_id_claim.clone(),
        config.oidc_display_name_claim.clone(),
        config.oidc_groups_claim.clone(),
    )
    .await
    .unwrap_or_else(|err| {
        fail(format!(
            "failed to discover the OIDC provider at {issuer_url}: {err}"
        ))
    });
    Some(Arc::new(provider))
}

/// Opens the blob store named by `ANAMNESIS_BLOB_ROOT`, picking the backend
/// from the value's scheme exactly as [`open_store`] picks a database driver
/// from `ANAMNESIS_DATABASE_URL`: an `s3://bucket/prefix` URL is an object
/// store, and anything else is a filesystem path.
async fn open_blob_store(config: &Config) -> Arc<dyn BlobStore> {
    let Some(s3) = &config.s3 else {
        let store = FsBlobStore::new(&config.blob_root)
            .await
            .unwrap_or_else(|err| {
                fail(format!(
                    "failed to prepare the blob store root {:?}: {err}",
                    config.blob_root
                ))
            });
        return Arc::new(store);
    };

    // `Config` only populates `s3` for an `s3://` root, and it fails at
    // startup if the credentials for one are missing -- so by here the URL
    // and the settings are known to agree.
    let settings = S3Settings {
        endpoint: s3.endpoint.clone(),
        region: s3.region.clone(),
        access_key_id: s3.access_key_id.clone(),
        secret_access_key: s3.secret_access_key.expose().to_string(),
    };
    let store = S3BlobStore::new(&config.blob_root, settings).unwrap_or_else(|err| {
        fail(format!(
            "failed to open the blob store at {:?}: {err}",
            config.blob_root
        ))
    });
    // Nothing has been contacted yet: unlike the database, an S3 endpoint has
    // no connection to open, so a wrong endpoint or credential first shows up
    // on an attachment upload rather than here.
    Arc::new(store)
}

/// Assembles the shared application state.
///
/// `SystemClock`, `UuidIdGen` and `TzTimezoneResolver` are stateless unit
/// structs with nothing to configure, so they are built here rather than
/// threaded in as parameters — they are not startup *inputs*, only the
/// concrete adapters this binary happens to pick.
fn build_state(
    config: &Config,
    store: Arc<SqlStore>,
    blobs: Arc<dyn BlobStore>,
    identity: Option<Arc<dyn IdentityProvider>>,
    leases: Arc<dyn JobLease>,
) -> AppState {
    AppState {
        areas: store.clone(),
        projects: store.clone(),
        tasks: store.clone(),
        relationships: store.clone(),
        tangles: store.clone(),
        comments: store.clone(),
        attachments: store.clone(),
        blobs,
        board: store.clone(),
        search: store.clone(),
        search_index: store.clone(),
        membership: store.clone(),
        membership_write: store.clone(),
        group_membership: store.clone(),
        group_membership_write: store.clone(),
        timezone: Arc::new(TzTimezoneResolver::new()),
        clock: Arc::new(SystemClock),
        id_gen: Arc::new(UuidIdGen),
        leases,
        identity,
        templates: Arc::new(templates::build_environment()),
        cookie_key: config.cookie_key.clone(),
        dev_auth_bypass: config.dev_auth_bypass,
        dev_csrf_token: session::generate_csrf_token(),
        secure_cookies: config.base_url_is_https(),
        max_body_bytes: config.max_body_bytes,
        settings: store,
        timezone_name: config.timezone.clone(),
    }
}

/// Binds `addr` and serves until SIGINT or SIGTERM, draining in-flight
/// requests.
async fn serve(app: Router, addr: SocketAddr) {
    // Installed before the socket is bound, so a signal arriving while the
    // listener is coming up is caught rather than killing the process.
    let shutdown = shutdown_signal();

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|err| fail(format!("failed to bind {addr}: {err}")));
    tracing::info!(addr = %addr, "anamnesis listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .expect("server encountered a fatal error");
}

/// Installs the shutdown-signal handlers and returns a future resolving once
/// the first of them fires.
///
/// SIGINT alone is not enough. Podman, Docker, Kubernetes and systemd all
/// stop a service with **SIGTERM**, whose default disposition kills the
/// process outright: every in-flight request is dropped, and the supervisor
/// then waits out its whole grace period for a process that is already gone.
///
/// Deliberately **not** `async`: the handlers are registered when this is
/// *called*, during startup, rather than the first time the returned future
/// is polled. Otherwise a signal arriving in the gap between binding the
/// socket and axum's first poll of the shutdown future would still be fatal.
#[cfg(unix)]
fn shutdown_signal() -> impl Future<Output = ()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate())
        .unwrap_or_else(|err| fail(format!("failed to install the SIGTERM handler: {err}")));

    async move {
        let signal = tokio::select! {
            _ = tokio::signal::ctrl_c() => "SIGINT",
            _ = terminate.recv() => "SIGTERM",
        };
        tracing::info!(
            signal,
            "shutdown signal received, draining in-flight requests"
        );
    }
}

/// The non-Unix fallback: there is no SIGTERM to select over, so Ctrl-C is
/// the only stop signal there is.
#[cfg(not(unix))]
fn shutdown_signal() -> impl Future<Output = ()> {
    async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!(
            signal = "SIGINT",
            "shutdown signal received, draining in-flight requests"
        );
    }
}
