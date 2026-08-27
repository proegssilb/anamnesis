//! New infrastructure ports named in `docs/DOMAIN.md` §7: `BlobStore`
//! (attachment files) and `SearchIndex` (keeping global search current).
//! Plus `TimezoneResolver`, needed to drive `anamnesis_core::next_run` and
//! the sweep but not itself named as a port in §7 — see the doc comment on
//! the trait for why it exists.

use async_trait::async_trait;

use anamnesis_core::Timestamp;
use time::{Date, Time};

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

/// Converts between UTC instants and local wall-clock calendar values in a
/// named IANA time zone (e.g. `"America/New_York"`), for whatever real tzdb
/// an adapter embeds.
///
/// **Why this port exists**: `anamnesis-core` deliberately carries no IANA
/// time zone database (`serde`, `thiserror`, `time`, `uuid` only —
/// `anamnesis_core::recurrence`'s module doc comment explains why: real DST
/// rules change by government decree, sometimes with only weeks of notice,
/// and a rule reapplied to a historical timestamp must use whichever rule
/// was in force *then*, not today's — both are exactly what a full tzdb
/// gets right and a hand-rolled rule table cannot). So `anamnesis_core`'s
/// `next_run` works purely in local calendar terms (a [`Date`] in, a
/// [`Date`] out) and never sees an offset. Turning a UTC instant into "what
/// local date is it" to get `next_run`'s input, and turning its answer (at
/// local midnight) back into a UTC instant to actually schedule the sweep,
/// needs a real tzdb — this port is that seam. An adapter backed by one
/// (Phase E) does the lookup and conversion; `anamnesis-core` stays free of
/// a timezone-database dependency and `anamnesis-app` stays free of a
/// *concrete* one (this is a trait, not an implementation).
///
/// The two methods are exactly the two conversions a sweep needs:
/// [`local_date`](Self::local_date) turns `Clock::now` into the `from` date
/// `next_run` wants, and [`to_utc`](Self::to_utc) turns the local midnight
/// `next_run` returns back into a [`Timestamp`] to schedule against and
/// compare `now` to.
///
/// Not `async`: every real implementation (an embedded tzdb) is an
/// in-memory lookup, not I/O.
pub trait TimezoneResolver: Send + Sync {
    /// The local calendar date `instant` falls on in the zone named
    /// `iana_name`. `Err` if `iana_name` is not a recognised zone.
    fn local_date(&self, iana_name: &str, instant: Timestamp) -> Result<Date, RepoError>;

    /// The UTC instant corresponding to local wall-clock `date` at `time` in
    /// the zone named `iana_name`. `Err` if `iana_name` is not a recognised
    /// zone, or if the given local date/time cannot be resolved to an
    /// instant there (e.g. it falls in a spring-forward gap that skips that
    /// wall-clock hour entirely).
    ///
    /// A local time that is *ambiguous* rather than nonexistent (the
    /// repeated hour of a fall-back transition) resolves to the earlier of
    /// the two matching instants — an adapter's doc comment should say so
    /// explicitly, since it is a real, if rare, choice.
    fn to_utc(&self, iana_name: &str, date: Date, time: Time) -> Result<Timestamp, RepoError>;
}
