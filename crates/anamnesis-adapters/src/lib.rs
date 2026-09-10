#![forbid(unsafe_code)]
//! `anamnesis-adapters`: concrete implementations of the ports declared in
//! `anamnesis-app`, for the real domain model (`docs/DOMAIN.md` §7, §10).
//!
//! - [`SqlStore`] — every Phase E port that talks to the relational schema:
//!   the per-entity repositories, `BoardQuery`, `SearchQuery`/`SearchIndex`,
//!   and `MembershipQuery` (`crate::sql`).
//! - [`FsBlobStore`] and [`S3BlobStore`] (attachment bytes on a local
//!   filesystem, or in an S3-compatible object store — `crate::blob_store`
//!   explains which is for what) and [`TzTimezoneResolver`] (a real IANA
//!   tzdb lookup) stand alone since none of them touches the SQL schema.
//! - `SystemClock`, `UuidIdGen`, and `OidcIdentityProvider` are the
//!   remaining shared infrastructure: a clock, an id generator, and OIDC.
//! - [`AesGcmTokenCipher`] and [`HttpIssueTrackerClient`] back project sync
//!   (issues #40/#41): encrypting a stored personal access token at rest,
//!   and talking to GitHub/Forgejo's REST API.

mod blob_store;
mod clock;
mod crypto;
mod id_gen;
mod identity;
mod issue_tracker;
mod sql;
mod timezone;

pub use blob_store::{FsBlobStore, S3BlobStore, S3Settings};
pub use clock::SystemClock;
pub use crypto::AesGcmTokenCipher;
pub use id_gen::UuidIdGen;
pub use identity::OidcIdentityProvider;
pub use issue_tracker::{HttpIssueTrackerClient, build_client};
pub use sql::{SqlJobLease, SqlStore};
pub use timezone::TzTimezoneResolver;
