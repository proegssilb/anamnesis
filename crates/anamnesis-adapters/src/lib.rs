#![forbid(unsafe_code)]
//! `anamnesis-adapters`: concrete implementations of the ports declared in
//! `anamnesis-app`, for the real domain model (`docs/DOMAIN.md` §7, §10).
//!
//! - [`SqlStore`] — every Phase E port that talks to the relational schema:
//!   the per-entity repositories, `BoardQuery`, `SearchQuery`/`SearchIndex`,
//!   and `MembershipQuery` (`crate::sql`).
//! - [`FsBlobStore`] (local filesystem attachments) and
//!   [`TzTimezoneResolver`] (a real IANA tzdb lookup) stand alone since
//!   neither touches the SQL schema.
//! - `SystemClock`, `UuidIdGen`, and `OidcIdentityProvider` are the
//!   remaining shared infrastructure: a clock, an id generator, and OIDC.

mod blob_store;
mod clock;
mod id_gen;
mod identity;
mod sql;
mod timezone;

pub use blob_store::FsBlobStore;
pub use clock::SystemClock;
pub use id_gen::UuidIdGen;
pub use identity::OidcIdentityProvider;
pub use sql::SqlStore;
pub use timezone::TzTimezoneResolver;
