#![forbid(unsafe_code)]
//! The binary entry point: resolve configuration, wire real adapters,
//! build the router, and serve it. All the actual logic lives in the
//! library (`src/lib.rs` and its modules) so integration tests can build
//! the same `Router` in-process without a socket.

use std::sync::Arc;

use axum_extra::extract::cookie::Key;
use tracing_subscriber::EnvFilter;

use anamnesis_adapters::{OidcIdentityProvider, SqlBoardRepository, SystemClock, UuidIdGen};
use anamnesis_app::IdentityProvider;
use anamnesis_web::config::Config;
use anamnesis_web::state::AppState;
use anamnesis_web::{routes, session, templates};

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

    let repo = SqlBoardRepository::connect(&config.database_url)
        .await
        .unwrap_or_else(|err| {
            eprintln!("failed to connect to the database: {err}");
            std::process::exit(1);
        });

    let identity: Option<Arc<dyn IdentityProvider>> = match &config.oidc_issuer_url {
        Some(issuer_url) => {
            let client_id = config
                .oidc_client_id
                .clone()
                .expect("ANAMNESIS_OIDC_CLIENT_ID is required whenever an issuer URL is set");
            let redirect_url = format!("{}/auth/callback", config.base_url.trim_end_matches('/'));
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
        None => None,
    };

    let state = AppState {
        repo: Arc::new(repo),
        clock: Arc::new(SystemClock),
        id_gen: Arc::new(UuidIdGen),
        identity,
        templates: Arc::new(templates::build_environment()),
        cookie_key: Key::from(config.session_secret.as_bytes()),
        dev_auth_bypass: config.dev_auth_bypass,
        dev_csrf_token: session::generate_csrf_token(),
        secure_cookies: config.base_url_is_https(),
    };

    let app = routes::build_router(state);

    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .unwrap_or_else(|err| {
            eprintln!("failed to bind {}: {err}", config.bind_addr);
            std::process::exit(1);
        });
    tracing::info!(addr = %config.bind_addr, "anamnesis listening");

    axum::serve(listener, app)
        .await
        .expect("server encountered a fatal error");
}
