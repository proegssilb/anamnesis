#![forbid(unsafe_code)]
//! `anamnesis-adapters`: concrete implementations of the ports declared in
//! `anamnesis-app` — a system clock, a UUID generator, a dual SQLite/Postgres
//! board repository, and an OIDC identity provider. See
//! `docs/ARCHITECTURE.md`.

mod board_repository;
mod clock;
mod id_gen;
mod identity;

pub use board_repository::SqlBoardRepository;
pub use clock::SystemClock;
pub use id_gen::UuidIdGen;
pub use identity::OidcIdentityProvider;
