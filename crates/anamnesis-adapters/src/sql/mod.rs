//! [`SqlStore`]: the dual SQLite/Postgres backend for every Phase E port
//! that talks to the relational schema (`docs/DOMAIN.md` §7) — every
//! per-entity repository, `BoardQuery`, `SearchQuery`/`SearchIndex`, and
//! `MembershipQuery`.
//!
//! One struct implements all of them, sharing one connection pool and one
//! migration run, for the same reason `tests/domain_fakes` is one `Fakes`
//! struct rather than a dozen disconnected mocks: several of these ports
//! must agree about the same underlying rows in the same transaction-free
//! reads (`BoardQuery` reading tasks a `TaskRepository` just wrote, a
//! `SearchQuery` reading what `SearchIndex` just indexed). Splitting them
//! into separate Rust types would not change the tables they share, only
//! add connection-pool bookkeeping.
//!
//! Each entity's SQL lives in its own submodule (`area`, `project`, `task`,
//! ...), following the legacy `board_repository`'s dual-backend pattern:
//! runtime `sqlx::query` (never `query!`), a private `sqlite_impl`/
//! `postgres_impl` pair per operation, and one shared contract test run
//! against both (`tests/`).

mod area;
mod attachment;
mod board_query;
mod comment;
mod lease;
mod membership;
mod project;
mod relationship;
mod search;
mod settings;
mod tangle;
mod task;

use std::str::FromStr;
use std::time::{Duration, Instant};

use anamnesis_app::{Clock, JobLease as _, RepoError};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{PgPool, SqlitePool};

use crate::clock::SystemClock;

pub use lease::SqlJobLease;

static SQLITE_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("migrations/sqlite");
static POSTGRES_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("migrations/postgres");

/// The two backends a connection string can select, exactly as
/// `crate::board_repository::Backend` does for the legacy repository.
#[derive(Debug)]
pub(crate) enum Backend {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}

/// Backs every Phase E port that talks to SQL: [`anamnesis_app::AreaRepository`],
/// [`anamnesis_app::ProjectRepository`], [`anamnesis_app::TaskRepository`],
/// [`anamnesis_app::RelationshipRepository`], [`anamnesis_app::TangleRepository`],
/// [`anamnesis_app::CommentRepository`], [`anamnesis_app::AttachmentRepository`],
/// [`anamnesis_app::BoardQuery`], [`anamnesis_app::SearchQuery`],
/// [`anamnesis_app::SearchIndex`], and [`anamnesis_app::MembershipQuery`].
#[derive(Debug)]
pub struct SqlStore {
    pub(crate) backend: Backend,
    /// Where job leases live — see [`lease_database_url`] for why this is a
    /// *second SQLite file* rather than another table in [`Self::backend`],
    /// and why on Postgres it is simply the same pool.
    pub(crate) leases: Backend,
}

impl SqlStore {
    /// Connects to `database_url`, running that backend's migrations before
    /// returning. `sqlite://` selects SQLite, `postgres://`/`postgresql://`
    /// selects Postgres; anything else is a startup error naming both
    /// supported forms.
    ///
    /// The migration run is taken under the [`MIGRATION_JOB`] lease, so that
    /// instances starting at the same moment migrate one at a time. That
    /// matters because sqlx's SQLite migrator is *not* safe to run
    /// concurrently: it lists the applied migrations outside any transaction
    /// and then applies the missing ones in separate transactions, so two
    /// processes can both read an empty list and both try to apply the same
    /// migration. The loser fails with `SQLITE_ERROR` — not `SQLITE_BUSY`, so
    /// no `busy_timeout` and no retry on lock contention covers it. (Postgres
    /// needs no such help: its migrator holds `pg_advisory_lock` across the
    /// whole run. Taking the lease there too costs one row and keeps a single
    /// startup path to reason about.)
    pub async fn connect(database_url: &str) -> Result<Self, RepoError> {
        let store = Self::open(database_url).await?;
        store.migrate().await?;
        Ok(store)
    }

    /// Opens the pools without touching any schema.
    async fn open(database_url: &str) -> Result<Self, RepoError> {
        if database_url.starts_with("sqlite://") {
            Ok(Self {
                backend: Backend::Sqlite(connect_sqlite(database_url).await?),
                leases: Backend::Sqlite(connect_sqlite(&lease_database_url(database_url)).await?),
            })
        } else if database_url.starts_with("postgres://")
            || database_url.starts_with("postgresql://")
        {
            let pool = PgPool::connect(database_url)
                .await
                .map_err(|e| RepoError::from_source("failed to connect to Postgres database", e))?;
            Ok(Self {
                leases: Backend::Postgres(pool.clone()),
                backend: Backend::Postgres(pool),
            })
        } else {
            Err(RepoError::new(format!(
                "unsupported database URL {database_url:?}: expected a \
                 \"sqlite://\" URL or a \"postgres://\"/\"postgresql://\" URL"
            )))
        }
    }

    /// Runs this backend's migrations while holding the [`MIGRATION_JOB`]
    /// lease, renewing it for as long as they take.
    ///
    /// The renewal is why the lease is worth having at all. Without it the TTL
    /// would have to exceed the slowest migration anyone will ever write,
    /// which is a bound nobody can honestly pick — and exceeding it would let
    /// a second instance start migrating alongside the first, silently.
    async fn migrate(&self) -> Result<(), RepoError> {
        let leases = self.job_lease().await?;
        let owner = uuid::Uuid::new_v4().to_string();
        await_migration_lease(&leases, &owner).await?;

        let renewal = spawn_renewal(leases.clone(), owner.clone());
        let outcome = self.run_migrations().await;
        renewal.abort();

        if let Err(err) = leases.release(MIGRATION_JOB, &owner).await {
            tracing::warn!(
                error = %err,
                "could not release the migration lease; it will expire on its own"
            );
        }
        outcome
    }

    async fn run_migrations(&self) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => SQLITE_MIGRATOR
                .run(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to run SQLite migrations", e)),
            Backend::Postgres(pool) => POSTGRES_MIGRATOR
                .run(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to run Postgres migrations", e)),
        }
    }
}

/// The lease name the migration run coordinates on.
const MIGRATION_JOB: &str = "migrations";

/// How long a migration claim lasts before another instance may take it.
///
/// Short, because it is renewed: this only ever bounds how long an instance
/// killed mid-migration blocks its replacement.
const MIGRATION_LEASE_TTL: Duration = Duration::from_secs(30);

/// How often the holder renews. A third of the TTL, so two consecutive
/// renewals can fail without the lease lapsing under a migration that is still
/// running.
const MIGRATION_RENEW: Duration = Duration::from_secs(10);

const MIGRATION_POLL: Duration = Duration::from_millis(250);

/// How long a waiting instance stays quiet before it starts saying what it is
/// waiting for. Long enough that an ordinary fast migration logs nothing.
const MIGRATION_REPORT: Duration = Duration::from_secs(30);

/// Blocks until this instance holds the migration lease.
///
/// Deliberately has no deadline, unlike `bootstrap::run`'s equivalent. That
/// one bounds its wait because the work it guards is a fixed handful of
/// queries; migrations have no such bound, so a timeout here could only ever
/// be a guess that turns someone's slow-but-fine upgrade into a failed
/// startup. Liveness comes from the lease instead: the holder either finishes
/// and releases, or dies and lets the TTL lapse. The remaining case — a holder
/// that is alive and still migrating — is precisely the one to wait out, since
/// on SQLite it holds the write lock and nothing else could proceed anyway.
async fn await_migration_lease(leases: &SqlJobLease, owner: &str) -> Result<(), RepoError> {
    let started = Instant::now();
    let mut next_report = started + MIGRATION_REPORT;
    loop {
        if leases
            .try_acquire(MIGRATION_JOB, owner, SystemClock.now(), MIGRATION_LEASE_TTL)
            .await?
        {
            if started.elapsed() >= MIGRATION_REPORT {
                tracing::info!(
                    waited_seconds = started.elapsed().as_secs(),
                    "the other instance finished migrating; continuing"
                );
            }
            return Ok(());
        }
        if Instant::now() >= next_report {
            tracing::info!(
                waited_seconds = started.elapsed().as_secs(),
                "another instance is migrating the database; waiting for it"
            );
            next_report += MIGRATION_REPORT;
        }
        tokio::time::sleep(MIGRATION_POLL).await;
    }
}

/// Keeps `owner`'s migration claim alive until the returned handle is aborted.
///
/// On SQLite this only works because the lease lives in its own file: a
/// renewal is a write, and a migration holds the *data* file's write lock for
/// its whole duration, so a lease table sharing that file could not be written
/// by anyone — including the holder doing the migrating. See
/// [`lease_database_url`].
fn spawn_renewal(leases: SqlJobLease, owner: String) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(MIGRATION_RENEW).await;
            match leases
                .try_acquire(
                    MIGRATION_JOB,
                    &owner,
                    SystemClock.now(),
                    MIGRATION_LEASE_TTL,
                )
                .await
            {
                Ok(true) => {}
                // Only reachable if renewal failed for long enough that the
                // lease lapsed and another instance claimed it — which means
                // two instances are now migrating at once. Nothing here can
                // undo that, but it must not happen silently.
                Ok(false) => tracing::error!(
                    "another instance took the migration lease while this one was still migrating"
                ),
                Err(err) => tracing::error!(error = %err, "could not renew the migration lease"),
            }
        }
    })
}

/// The URL of the SQLite file job leases live in: the data file's name with
/// `-leases` appended before its extension, keeping any query string.
///
/// **Why a second file rather than a second table.** SQLite's write lock is
/// database-wide, so a lease row inside the data file cannot be written by
/// anyone while a migration — or any other long write — holds that lock. Not
/// even by the process holding it: a second connection from the same pool
/// waits out `busy_timeout` and then fails with `SQLITE_BUSY`. That makes
/// renewal impossible exactly when it matters, and it also means the sweep and
/// bootstrap leases would be unavailable precisely when the system is busiest,
/// which is the opposite of what a coordination table is for.
///
/// Postgres has no such problem — row-level MVCC means writing a lease never
/// contends with anything else — so it keeps one database, and this function
/// is not used for it. The asymmetry is a difference in the backends' locking
/// granularity, absorbed here rather than leaked upward.
///
/// The path is derived rather than configured so that the two files always
/// travel together; a lease file can never be paired with the wrong data file.
/// It holds no durable value — it is coordination state, disposable once every
/// instance is stopped — but it must not be deleted while any are running.
fn lease_database_url(database_url: &str) -> String {
    let (path, query) = match database_url.split_once('?') {
        Some((path, query)) => (path, format!("?{query}")),
        None => (database_url, String::new()),
    };
    let segment = path.rfind('/').map_or(0, |slash| slash + 1);
    let extension = path[segment..]
        .rfind('.')
        .map_or(path.len(), |d| segment + d);
    format!("{}-leases{}{query}", &path[..extension], &path[extension..])
}

/// Builds the SQLite pool with its pragmas chosen here rather than inherited
/// from whatever sqlx or the existing file happens to default to.
///
/// All three matter once more than one process shares the file:
///
/// - **WAL journal mode.** sqlx deliberately sets no journal mode of its own,
///   so without this we get whatever the file already has — `DELETE` for a
///   fresh database, under which a writer blocks every reader. WAL persists in
///   the file, so this is effectively a one-time conversion.
/// - **`synchronous = NORMAL`.** sqlx defaults to `FULL`, which fsyncs on every
///   commit. Under WAL, `NORMAL` risks losing only the most recent
///   transactions on an OS-level crash — not corruption, and not on a mere
///   process crash — in exchange for not fsyncing per commit.
/// - **`busy_timeout`.** Five seconds is also sqlx's default, but it is stated
///   here because it is load-bearing rather than incidental: it is what makes
///   two processes' ordinary concurrent writes queue instead of one failing
///   outright with `SQLITE_BUSY`. Note what it does *not* cover — sqlx's
///   SQLite migrator takes no lock at all (its `lock`/`unlock` are empty), and
///   its unprotected check-then-act fails with `SQLITE_ERROR` rather than
///   `SQLITE_BUSY`, so no timeout can rescue it. That is
///   [`SqlStore::migrate`]'s lease's job, not this setting's.
///
/// The journal-mode conversion is then retried, because `busy_timeout` does
/// not cover that either — see [`WAL_RETRY_WINDOW`].
async fn connect_sqlite(database_url: &str) -> Result<SqlitePool, RepoError> {
    let options = SqliteConnectOptions::from_str(database_url)
        .map_err(|e| RepoError::from_source("invalid SQLite database URL", e))?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(BUSY_TIMEOUT);

    let deadline = std::time::Instant::now() + WAL_RETRY_WINDOW;
    loop {
        match SqlitePool::connect_with(options.clone()).await {
            Ok(pool) => return Ok(pool),
            Err(e) if is_busy(&e) && std::time::Instant::now() < deadline => {
                tokio::time::sleep(WAL_RETRY_PAUSE).await;
            }
            Err(e) => {
                return Err(RepoError::from_source(
                    "failed to connect to SQLite database",
                    e,
                ));
            }
        }
    }
}

/// How long sqlx waits out an ordinary lock before giving up. See
/// [`connect_sqlite`] for why this is stated rather than inherited.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// How long [`connect_sqlite`] keeps retrying a connection that lost the race
/// to convert the database to WAL, and how long it pauses between attempts.
///
/// [`BUSY_TIMEOUT`] does not cover this. Switching journal modes needs a brief
/// exclusive lock on the whole database, and SQLite does *not* invoke the busy
/// handler while trying to take it — `PRAGMA journal_mode = WAL` returns
/// `SQLITE_BUSY` immediately if any other connection has the file open. Two
/// processes starting against one fresh database therefore race, and one of
/// them simply loses: without this loop, that instance fails to start at all,
/// which is precisely the same-machine multi-process topology
/// (`docs/DEPLOYMENT.md` §12) at its very first moment.
///
/// Retrying is the right answer rather than a papering-over, because the
/// outcome we want still happens — the *winner* converts the file, and WAL
/// persists in it. The loser's retry then finds a database that is already in
/// WAL, where the pragma is a no-op needing no exclusive lock, so the loop
/// converges on the first retry rather than grinding out the window. It also
/// means that once this function returns `Ok`, the file is known to be in WAL,
/// so the connections the pool opens lazily later can never hit this at all.
const WAL_RETRY_WINDOW: Duration = Duration::from_secs(5);

const WAL_RETRY_PAUSE: Duration = Duration::from_millis(25);

/// Whether `e` is SQLite reporting a lock it could not get.
///
/// Matched on the result code rather than the message, masked to the low byte
/// so the extended codes (`SQLITE_BUSY_RECOVERY`, `SQLITE_BUSY_SNAPSHOT`,
/// `SQLITE_BUSY_TIMEOUT`) count as the plain `SQLITE_BUSY` they refine.
fn is_busy(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .and_then(|db| db.code())
        .and_then(|code| code.parse::<i32>().ok())
        .is_some_and(|code| code & 0xff == 5)
}

/// Parses a stored SQLite `TEXT` id column back into a `Uuid`.
pub(crate) fn parse_uuid(raw: &str) -> Result<uuid::Uuid, RepoError> {
    uuid::Uuid::parse_str(raw).map_err(|e| RepoError::from_source("invalid stored id", e))
}

/// Builds a validated [`anamnesis_core::Title`] from stored text, wrapping a
/// validation failure as a [`RepoError`] — a stored row failing a rule the
/// core type enforces on construction indicates corrupt data, not a normal
/// "not found".
pub(crate) fn title_from_text(raw: String) -> Result<anamnesis_core::Title, RepoError> {
    anamnesis_core::Title::new(&raw)
        .map_err(|e| RepoError::from_source(format!("invalid stored title {raw:?}"), e))
}

pub(crate) fn timestamp_from_seconds(raw: i64) -> Result<anamnesis_core::Timestamp, RepoError> {
    anamnesis_core::Timestamp::from_unix_seconds(raw)
        .map_err(|e| RepoError::from_source("invalid stored timestamp", e))
}

/// `ProjectStatus` <-> its stored text representation.
pub(crate) fn project_status_to_text(status: anamnesis_core::ProjectStatus) -> &'static str {
    use anamnesis_core::ProjectStatus::*;
    match status {
        Pending => "pending",
        Active => "active",
        Complete => "complete",
    }
}

pub(crate) fn project_status_from_text(
    raw: &str,
) -> Result<anamnesis_core::ProjectStatus, RepoError> {
    use anamnesis_core::ProjectStatus::*;
    match raw {
        "pending" => Ok(Pending),
        "active" => Ok(Active),
        "complete" => Ok(Complete),
        other => Err(RepoError::new(format!(
            "invalid stored project status {other:?}"
        ))),
    }
}

/// `FieldKind` <-> its stored text representation.
pub(crate) fn field_kind_to_text(kind: anamnesis_core::FieldKind) -> &'static str {
    use anamnesis_core::FieldKind::*;
    match kind {
        Number => "number",
        Currency => "currency",
        Date => "date",
        Time => "time",
        DateTime => "datetime",
        Line => "line",
        Block => "block",
    }
}

/// `time::Weekday` <-> its stored text representation, for
/// `Recurrence::EveryNWeeks`'s `weekday` field.
pub(crate) fn weekday_to_text(weekday: time::Weekday) -> &'static str {
    use time::Weekday::*;
    match weekday {
        Monday => "monday",
        Tuesday => "tuesday",
        Wednesday => "wednesday",
        Thursday => "thursday",
        Friday => "friday",
        Saturday => "saturday",
        Sunday => "sunday",
    }
}

pub(crate) fn weekday_from_text(raw: &str) -> Result<time::Weekday, RepoError> {
    use time::Weekday::*;
    match raw {
        "monday" => Ok(Monday),
        "tuesday" => Ok(Tuesday),
        "wednesday" => Ok(Wednesday),
        "thursday" => Ok(Thursday),
        "friday" => Ok(Friday),
        "saturday" => Ok(Saturday),
        "sunday" => Ok(Sunday),
        other => Err(RepoError::new(format!("invalid stored weekday {other:?}"))),
    }
}

pub(crate) fn field_kind_from_text(raw: &str) -> Result<anamnesis_core::FieldKind, RepoError> {
    use anamnesis_core::FieldKind::*;
    match raw {
        "number" => Ok(Number),
        "currency" => Ok(Currency),
        "date" => Ok(Date),
        "time" => Ok(Time),
        "datetime" => Ok(DateTime),
        "line" => Ok(Line),
        "block" => Ok(Block),
        other => Err(RepoError::new(format!(
            "invalid stored field kind {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lease_file_sits_beside_the_data_file() {
        assert_eq!(
            lease_database_url("sqlite:///var/lib/anamnesis/anamnesis.db"),
            "sqlite:///var/lib/anamnesis/anamnesis-leases.db"
        );
    }

    #[test]
    fn the_query_string_is_carried_over() {
        // `?mode=rwc` is what creates the file on first boot, so losing it
        // would break exactly the case the lease exists for.
        assert_eq!(
            lease_database_url("sqlite:///tmp/anamnesis.db?mode=rwc"),
            "sqlite:///tmp/anamnesis-leases.db?mode=rwc"
        );
    }

    #[test]
    fn a_dot_in_a_directory_name_is_not_mistaken_for_an_extension() {
        assert_eq!(
            lease_database_url("sqlite:///srv/app.v2/anamnesis"),
            "sqlite:///srv/app.v2/anamnesis-leases"
        );
    }
}
