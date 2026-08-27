#![forbid(unsafe_code)]
//! The binary entry point: resolve configuration, wire real adapters,
//! bootstrap a fresh database, build the router, and serve it. All the
//! actual logic lives in the library (`src/lib.rs` and its modules) so
//! integration tests can build the same `Router` in-process without a
//! socket.

use std::sync::Arc;

use axum_extra::extract::cookie::Key;
use tracing_subscriber::EnvFilter;

use anamnesis_adapters::{
    FsBlobStore, OidcIdentityProvider, SqlStore, SystemClock, TzTimezoneResolver, UuidIdGen,
};
use anamnesis_app::{Clock, IdentityProvider, TimezoneResolver};
use anamnesis_web::config::Config;
use anamnesis_web::state::AppState;
use anamnesis_web::{bootstrap, routes, session, sweep, templates};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env().unwrap_or_else(|err| {
        eprintln!("configuration error: {err}");
        std::process::exit(1);
    });

    if config.dev_auth_bypass {
        tracing::warn!(
            "ANAMNESIS_DEV_AUTH_BYPASS is enabled: every request is authenticated as a fixed \
             local user with no identity provider involved. This must never be set in a real \
             deployment."
        );
    }

    let store = SqlStore::connect(&config.database_url)
        .await
        .unwrap_or_else(|err| {
            eprintln!("failed to connect to the database: {err}");
            std::process::exit(1);
        });

    let clock = SystemClock;
    let id_gen = UuidIdGen;
    let timezone = TzTimezoneResolver::new();

    // Fail loudly, naming the variable, if ANAMNESIS_TIMEZONE does not name
    // a real IANA zone -- checked once, at startup, against "now", rather
    // than discovered lazily the first time a sweep or a suggestion needs
    // it.
    if let Err(err) = timezone.local_date(&config.timezone, clock.now()) {
        eprintln!("invalid ANAMNESIS_TIMEZONE {:?}: {err}", config.timezone);
        std::process::exit(1);
    }

    bootstrap::run(&store, &id_gen, &config.bootstrap_admin, &config.timezone)
        .await
        .unwrap_or_else(|err| {
            eprintln!("failed to bootstrap the database: {err}");
            std::process::exit(1);
        });

    // Dev bypass never needs a real provider — skip discovery entirely even
    // if ANAMNESIS_OIDC_* happens to also be set (config.rs only makes those
    // optional in bypass mode, it does not forbid setting them), so a stray
    // or unreachable issuer URL can never break a `cargo run` with bypass on.
    let identity: Option<Arc<dyn IdentityProvider>> =
        match (&config.oidc_issuer_url, config.dev_auth_bypass) {
            (_, true) => None,
            (None, false) => None,
            (Some(issuer_url), false) => {
                let client_id = config
                    .oidc_client_id
                    .clone()
                    .expect("ANAMNESIS_OIDC_CLIENT_ID is required whenever an issuer URL is set");
                let redirect_url =
                    format!("{}/auth/callback", config.base_url.trim_end_matches('/'));
                let provider = OidcIdentityProvider::discover(
                    issuer_url,
                    client_id,
                    config.oidc_client_secret.clone(),
                    redirect_url,
                    config.oidc_scopes.clone(),
                )
                .await
                .unwrap_or_else(|err| {
                    eprintln!("failed to discover the OIDC provider at {issuer_url}: {err}");
                    std::process::exit(1);
                });
                Some(Arc::new(provider))
            }
        };

    let blobs = FsBlobStore::new(&config.blob_root)
        .await
        .unwrap_or_else(|err| {
            eprintln!(
                "failed to prepare the blob store root {:?}: {err}",
                config.blob_root
            );
            std::process::exit(1);
        });

    let store = Arc::new(store);
    let state = AppState {
        areas: store.clone(),
        projects: store.clone(),
        tasks: store.clone(),
        relationships: store.clone(),
        tangles: store.clone(),
        comments: store.clone(),
        attachments: store.clone(),
        blobs: Arc::new(blobs),
        board: store.clone(),
        search: store.clone(),
        search_index: store.clone(),
        membership: store.clone(),
        timezone: Arc::new(timezone),
        clock: Arc::new(clock),
        id_gen: Arc::new(id_gen),
        identity,
        templates: Arc::new(templates::build_environment()),
        cookie_key: Key::from(config.session_secret.as_bytes()),
        dev_auth_bypass: config.dev_auth_bypass,
        dev_csrf_token: session::generate_csrf_token(),
        secure_cookies: config.base_url_is_https(),
        settings: store.clone(),
        timezone_name: config.timezone.clone(),
    };

    // The scheduled-sweep ticker (`docs/DOMAIN.md` §6). Deliberately started
    // only here, in the binary -- never from `routes::build_router`,
    // `AppState` construction, or `bootstrap::run` -- so no integration test
    // (which builds a `Router` directly via `routes::build_router`, per
    // `tests/support`) can ever cause it to spawn. See `sweep`'s module doc
    // comment for the full reasoning.
    let ticker_handle = sweep::spawn_ticker(state.clone());

    let app = routes::build_router(state);

    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .unwrap_or_else(|err| {
            eprintln!("failed to bind {}: {err}", config.bind_addr);
            std::process::exit(1);
        });
    tracing::info!(addr = %config.bind_addr, "anamnesis listening");

    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutdown signal received, draining in-flight requests");
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .expect("server encountered a fatal error");

    // The ticker is a detached background task with nothing left to flush
    // (a sweep either committed or it didn't; `sweep_done` is idempotent, so
    // an abort mid-sweep is safe to resume on the next boot -- see `sweep`'s
    // module doc comment) -- `abort()` returns immediately rather than
    // waiting for its next wake-up, so it never delays process exit.
    ticker_handle.abort();
}
