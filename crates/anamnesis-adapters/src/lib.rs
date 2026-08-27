#![forbid(unsafe_code)]
//! `anamnesis-adapters`: concrete implementations of the ports declared in
//! `anamnesis-app`.
//!
//! Two generations live here side by side (`docs/DOMAIN.md` §7, §10):
//! - [`SqlBoardRepository`] — the legacy kanban scaffold's single-aggregate
//!   `BoardRepository`, kept compiling for `anamnesis-web` until Phase F.
//! - [`SqlStore`] — every Phase E port for the real domain model: the
//!   per-entity repositories, `BoardQuery`, `SearchQuery`/`SearchIndex`, and
//!   `MembershipQuery` (`crate::sql`), plus [`FsBlobStore`] (local
//!   filesystem attachments) and [`TableTimezoneResolver`] (IANA zone name
//!   lookup) standing alone since neither touches the SQL schema.
//!
//! `SystemClock`, `UuidIdGen`, and `OidcIdentityProvider` are shared
//! infrastructure used by both generations.

mod blob_store;
mod board_repository;
mod clock;
mod id_gen;
mod identity;
mod sql;
mod timezone;

pub use blob_store::FsBlobStore;
pub use board_repository::SqlBoardRepository;
pub use clock::SystemClock;
pub use id_gen::UuidIdGen;
pub use identity::OidcIdentityProvider;
pub use sql::SqlStore;
pub use timezone::TableTimezoneResolver;
