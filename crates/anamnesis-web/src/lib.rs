#![forbid(unsafe_code)]
//! `anamnesis-web`: the binary's library surface. Everything lives here so
//! integration tests (`tests/`) can build a real `axum::Router` in-process
//! with `tower::ServiceExt::oneshot`, exactly as the real binary would serve
//! it — `src/main.rs` is a thin wrapper that just wires this up to a real
//! socket. See `docs/ARCHITECTURE.md` and `docs/PLAN.md` (Phase 4).

pub mod auth;
pub mod bootstrap;
pub mod config;
pub mod error;
pub mod handlers;
pub mod hx;
pub mod routes;
pub mod session;
pub mod state;
pub mod static_files;
pub mod sweep;
pub mod templates;
