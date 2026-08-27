//! New infrastructure ports named in `docs/DOMAIN.md` §7: `BlobStore`
//! (attachment files) and `SearchIndex` (keeping global search current).
//! Plus `TimezoneResolver`, needed by Phase C's `next_run` but not itself
//! named as a port in §7 — see the doc comment on the trait for why it
//! exists.

use async_trait::async_trait;

use anamnesis_core::Timezone;

use crate::error::RepoError;

/// Stores and retrieves attachment file bytes (`docs/DOMAIN.md` §3: "Files
/// need a new `BlobStore` port (local filesystem first, S3-shaped later)").
/// Keyed by an opaque string the caller mints and records as an
/// [`crate::entities::AttachmentKind::File`]'s `blob_key` — this port does
/// not know or care what that string means to a given adapter (a filesystem
/// path, an S3 object key, ...).
#[async_trait]
pub trait BlobStore: Send + Sync {
    async fn put(&self, key: &str, bytes: Vec<u8>, mime: &str) -> Result<(), RepoError>;
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, RepoError>;
    async fn delete(&self, key: &str) -> Result<(), RepoError>;
}

/// Keeps a [`crate::ports::SearchQuery`] index current as areas, projects,
/// and tasks change. Split from `SearchQuery` (the read side) because the
/// two backends' write paths diverge as much as their read paths do
/// (`docs/DOMAIN.md` §7: "SQLite FTS5 vs Postgres `tsvector`/GIN").
#[async_trait]
pub trait SearchIndex: Send + Sync {
    async fn index_area(&self, id: anamnesis_core::AreaId, title: &str) -> Result<(), RepoError>;
    async fn index_project(
        &self,
        id: anamnesis_core::ProjectId,
        title: &str,
    ) -> Result<(), RepoError>;
    async fn index_task(&self, id: anamnesis_core::TaskId, title: &str) -> Result<(), RepoError>;
    async fn remove_area(&self, id: anamnesis_core::AreaId) -> Result<(), RepoError>;
    async fn remove_project(&self, id: anamnesis_core::ProjectId) -> Result<(), RepoError>;
    async fn remove_task(&self, id: anamnesis_core::TaskId) -> Result<(), RepoError>;
}

/// Resolves an IANA time zone name (e.g. `"America/New_York"`) to
/// [`anamnesis_core`]'s pure `Timezone` rule data (a standard offset plus an
/// optional DST rule — see `anamnesis_core::recurrence`'s module doc
/// comment).
///
/// **Why this port exists**: `anamnesis-core` deliberately carries no IANA
/// time zone database (`serde`, `thiserror`, `time`, `uuid` only — see the
/// `recurrence` module's doc comment on what Phase C did and did not add as
/// a dependency). Its `Timezone` type is pure rule data by design, but
/// hand-constructing one — a standard `UtcOffset` plus a `DstRule` spelled
/// out as "the Nth (or last) weekday of a month, at a given local time",
/// twice, once for the start and once for the end of daylight saving — is
/// roughly eighteen lines of Rust. No administrator can type that into a
/// settings field. This port is the seam: an adapter (Phase E) backed by a
/// real tzdata source does the lookup and hands back the finished value,
/// while `anamnesis-core` stays free of a timezone-database dependency and
/// `anamnesis-app` stays free of a *concrete* one (this is a trait, not an
/// implementation — "Define the port here; do not implement it").
///
/// Not `async`: every real implementation (an embedded tzdata table, e.g.
/// via `tzdb`) is an in-memory lookup, not I/O.
pub trait TimezoneResolver: Send + Sync {
    /// Resolves `iana_name` (e.g. `"America/New_York"`, `"Europe/Berlin"`)
    /// to its `Timezone` rule data, or `None` if the name is not recognised.
    fn resolve(&self, iana_name: &str) -> Option<Timezone>;
}
