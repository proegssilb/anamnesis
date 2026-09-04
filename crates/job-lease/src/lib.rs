//! Expiry-based leases in an ordinary SQL table, so that exactly one process
//! out of several runs a given named job.
//!
//! The problem this solves: you have a web app that runs some recurring work
//! on a timer — an archive sweep, a reindex, a garbage collection — and you
//! want to run more than one instance of that app for availability. Suddenly
//! every instance fires the timer, and the work happens N times.
//!
//! Existing options do not fit that shape well. Full job queues (`apalis`,
//! `sqlxmq`, `underway`) solve scheduling and retries, which is more machinery
//! than mutual exclusion needs — and they only relocate the problem, since
//! something must still decide which instance *enqueues* each scheduled tick.
//! `kube-lease-manager` does leader election properly but wants a Kubernetes
//! cluster. Consensus crates (`openraft`, `hiqlite`) are the right answer when
//! there is no shared store, and redundant when there already is one.
//!
//! If your instances already share a database, that database is a perfectly
//! good coordinator, and this crate is the ~200 lines that make it one.
//!
//! # Which topologies this works for
//!
//! Anything where every instance can see the same rows:
//!
//! - Postgres, any number of machines.
//! - SQLite, any number of processes **on one machine**, sharing the file.
//!
//! It does *not* work for replicated SQLite where each instance has its own
//! file, because a lease row only coordinates the instances that can see it.
//! Each replica would win its own private lease. Use a consensus-based tool
//! for that topology.
//!
//! # Why expiry and not a held lock
//!
//! A lease expires on its own, so a holder that crashes mid-job releases it by
//! doing nothing. The alternative — a connection-scoped lock such as Postgres'
//! `pg_advisory_lock` — leaks when a pooled connection is returned or when a
//! future is cancelled at a `tokio::time::timeout`, and cannot be released in
//! `Drop` because releasing requires an `await`.
//!
//! The cost is that a lease is only as good as the clocks involved. Pick a TTL
//! comfortably longer than the job's worst-case runtime, and treat the job
//! itself as idempotent anyway.
//!
//! # Usage
//!
//! You build the pool; this crate borrows it. That is deliberate — pool
//! configuration is a deployment concern, and a library that took it over
//! would be making your decisions for you. See [`SqlLease::sqlite`] for one
//! setting you must get right on SQLite.
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use std::time::Duration;
//! use job_lease::SqlLease;
//!
//! let pool = sqlx::SqlitePool::connect("sqlite://app.db").await?;
//! let leases = SqlLease::sqlite(pool).await?;
//!
//! let now = 1_760_000_000; // unix seconds, from whatever clock you trust
//! if leases.try_acquire("archive_sweep", "instance-a", now, Duration::from_secs(300)).await? {
//!     // ... do the work; no other instance is doing it ...
//!     leases.release("archive_sweep", "instance-a").await?;
//! }
//! # Ok(())
//! # }
//! ```
//!
//! `now` is a parameter rather than something this crate reads, so that a
//! caller with a testable clock abstraction can keep using it, and so tests
//! can advance time without sleeping.

use std::time::Duration;

use async_trait::async_trait;
use sqlx::migrate::{AppliedMigration, Migrate, MigrateError, Migration, Migrator};
use sqlx::{PgPool, SqlitePool};

/// This crate's own migrations table, kept separate from the default
/// `_sqlx_migrations` so that a consumer's migrations and this crate's do not
/// collide in the same database.
const MIGRATIONS_TABLE: &str = "_job_lease_migrations";

/// Anything that can go wrong claiming or releasing a lease.
#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    #[error("job-lease query failed: {0}")]
    Query(#[from] sqlx::Error),
    #[error("job-lease schema migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

/// The lease operations, as a trait, so consumers can substitute a fake in
/// tests.
///
/// [`SqlLease`] implements this, but you are not obliged to use it: its
/// inherent methods have the same signatures, and an application with its own
/// port conventions will usually be better off wrapping [`SqlLease`] in its
/// own trait than adopting this one.
#[async_trait]
pub trait JobLease: Send + Sync {
    /// Claims `job` for `owner` until `now + ttl`, returning whether the claim
    /// succeeded. See [`SqlLease::try_acquire`] for the exact semantics.
    async fn try_acquire(
        &self,
        job: &str,
        owner: &str,
        now: i64,
        ttl: Duration,
    ) -> Result<bool, LeaseError>;

    /// Releases `job` if `owner` still holds it. See [`SqlLease::release`].
    async fn release(&self, job: &str, owner: &str) -> Result<(), LeaseError>;
}

#[derive(Debug, Clone)]
enum Backend {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}

/// A lease store backed by a `job_leases` table in your database.
///
/// Cheap to clone — it holds a connection pool, which is itself a handle.
#[derive(Debug, Clone)]
pub struct SqlLease {
    backend: Backend,
}

impl SqlLease {
    /// Wraps a SQLite pool, creating the `job_leases` table if it is absent.
    ///
    /// Safe to call from several processes against one database file at the
    /// same time, which matters more here than it would for an ordinary
    /// schema: this table is what every other job in the system coordinates
    /// on, so it is the one piece that cannot itself be guarded by a lease.
    /// It is made safe directly instead — see `migrate_sqlite`, which does
    /// not use sqlx's own migrator and explains why.
    ///
    /// **Set `busy_timeout` on the pool** if more than one process will use
    /// this database. It is what makes a process that finds another one
    /// mid-migration wait for it rather than fail outright with
    /// `SQLITE_BUSY`. WAL journal mode is also worth setting for concurrent
    /// writers. Neither is this crate's call to make, which is why the pool
    /// is yours to build.
    pub async fn sqlite(pool: SqlitePool) -> Result<Self, LeaseError> {
        let mut migrator = sqlx::migrate!("migrations/sqlite");
        migrator.dangerous_set_table_name(MIGRATIONS_TABLE);
        migrate_sqlite(&pool, &migrator).await?;
        Ok(Self {
            backend: Backend::Sqlite(pool),
        })
    }

    /// Wraps a Postgres pool, creating the `job_leases` table if it is absent.
    ///
    /// Concurrent first starts are safe without any extra care: sqlx's
    /// Postgres migrator holds an advisory lock, keyed on the database name,
    /// for the whole run.
    pub async fn postgres(pool: PgPool) -> Result<Self, LeaseError> {
        let mut migrator = sqlx::migrate!("migrations/postgres");
        migrator.dangerous_set_table_name(MIGRATIONS_TABLE);
        migrator.run(&pool).await?;
        Ok(Self {
            backend: Backend::Postgres(pool),
        })
    }

    /// Claims `job` for `owner` until `now + ttl`.
    ///
    /// Returns `true` if `owner` now holds the lease, `false` if someone
    /// else's claim is still live. `now` is unix seconds, from whatever clock
    /// the caller trusts.
    ///
    /// Renewal is the same call: an owner that already holds `job` always
    /// succeeds and pushes its own expiry out, so a long-running job can
    /// extend its claim on a heartbeat without a distinct method.
    ///
    /// One statement, so the check and the claim cannot interleave with
    /// another instance's.
    pub async fn try_acquire(
        &self,
        job: &str,
        owner: &str,
        now: i64,
        ttl: Duration,
    ) -> Result<bool, LeaseError> {
        let expires_at = now.saturating_add(ttl_seconds(ttl));
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlite_impl::try_acquire(pool, job, owner, now, expires_at).await
            }
            Backend::Postgres(pool) => {
                postgres_impl::try_acquire(pool, job, owner, now, expires_at).await
            }
        }
    }

    /// Releases `job` if `owner` still holds it, so the next claimant does not
    /// have to wait out the remaining TTL.
    ///
    /// A no-op if the lease has already expired or been taken over — releasing
    /// someone else's lease is never possible. Failing to call this is safe;
    /// it only costs the rest of the TTL.
    pub async fn release(&self, job: &str, owner: &str) -> Result<(), LeaseError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::release(pool, job, owner).await,
            Backend::Postgres(pool) => postgres_impl::release(pool, job, owner).await,
        }
    }
}

#[async_trait]
impl JobLease for SqlLease {
    async fn try_acquire(
        &self,
        job: &str,
        owner: &str,
        now: i64,
        ttl: Duration,
    ) -> Result<bool, LeaseError> {
        SqlLease::try_acquire(self, job, owner, now, ttl).await
    }

    async fn release(&self, job: &str, owner: &str) -> Result<(), LeaseError> {
        SqlLease::release(self, job, owner).await
    }
}

/// Saturating, because a TTL longer than `i64::MAX` seconds and a TTL of
/// `i64::MAX` seconds mean the same thing in practice: never expires.
fn ttl_seconds(ttl: Duration) -> i64 {
    i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX)
}

/// Applies `migrator`'s migrations to SQLite, each inside its own
/// `BEGIN IMMEDIATE` transaction that re-checks whether that migration has
/// already been applied.
///
/// **Why not `Migrator::run`.** It reads the applied-version list once,
/// outside any transaction, and only then applies each missing migration in a
/// transaction of its own. That is a check-then-act with a gap in the middle:
/// two processes starting together both read an empty list, both decide the
/// first migration is pending, and both run it. The loser fails with
/// `SQLITE_ERROR: table job_leases already exists` — note `SQLITE_ERROR`, not
/// `SQLITE_BUSY`, so no `busy_timeout` and no retry loop covers it. Nothing
/// else stands in the way either: sqlx's SQLite migrator takes no lock at all,
/// its `Migrate::lock` and `unlock` being empty. (Postgres is unaffected —
/// its migrator holds an advisory lock across the whole run, which is why
/// [`SqlLease::postgres`] can just call `run`.)
///
/// Moving the read inside the transaction that does the write is what closes
/// the gap — see [`apply_one_migration`], which is where that happens.
///
/// **Per migration, not one transaction around the whole run.** Both are
/// correct, but a single outer transaction would hold the write lock for the
/// entire run, and migrations are not bounded-time. Under WAL that blocks
/// every other writer — including an instance still serving traffic during a
/// rolling upgrade — for as long as the slowest run takes. Per migration, the
/// lock is held for one migration's own work and released in between, which is
/// the floor that migration inherently costs rather than a ceiling this
/// function imposes.
async fn migrate_sqlite(pool: &SqlitePool, migrator: &Migrator) -> Result<(), LeaseError> {
    let pending = migrator
        .iter()
        .filter(|m| !m.migration_type.is_down_migration());
    for migration in pending {
        apply_one_migration(pool, migration).await?;
    }
    Ok(())
}

/// Applies `migration` if it is not already recorded, inside a single
/// `BEGIN IMMEDIATE` transaction.
///
/// `BEGIN IMMEDIATE` is what makes the check-then-act atomic: it takes
/// SQLite's write lock up front, so the applied-version list is read under the
/// same lock that the `apply` will write beneath. A plain `BEGIN` is deferred
/// and would not take the lock until the first write, which is already too
/// late.
///
/// A dirty version aborts the run — one of the two checks `Migrator::run`
/// would have made, kept deliberately.
async fn apply_one_migration(pool: &SqlitePool, migration: &Migration) -> Result<(), LeaseError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let conn = &mut *tx;
    conn.ensure_migrations_table(MIGRATIONS_TABLE).await?;
    if let Some(version) = conn.dirty_version(MIGRATIONS_TABLE).await? {
        return Err(MigrateError::Dirty(version).into());
    }
    let applied = conn.list_applied_migrations(MIGRATIONS_TABLE).await?;
    if !is_already_applied(&applied, migration)? {
        conn.apply(MIGRATIONS_TABLE, migration).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Whether `migration` is already recorded in `applied`.
///
/// The second check `Migrator::run` would have made: a recorded migration
/// whose checksum no longer matches the file raises
/// [`MigrateError::VersionMismatch`] rather than being silently skipped.
fn is_already_applied(
    applied: &[AppliedMigration],
    migration: &Migration,
) -> Result<bool, LeaseError> {
    match applied.iter().find(|a| a.version == migration.version) {
        Some(a) if a.checksum != migration.checksum => {
            Err(MigrateError::VersionMismatch(migration.version).into())
        }
        Some(_) => Ok(true),
        None => Ok(false),
    }
}

mod sqlite_impl {
    use super::{LeaseError, SqlitePool};

    pub(super) async fn try_acquire(
        pool: &SqlitePool,
        job: &str,
        owner: &str,
        now: i64,
        expires_at: i64,
    ) -> Result<bool, LeaseError> {
        let result = sqlx::query(
            "INSERT INTO job_leases (job_name, owner, expires_at) VALUES (?, ?, ?) \
             ON CONFLICT (job_name) DO UPDATE \
             SET owner = excluded.owner, expires_at = excluded.expires_at \
             WHERE job_leases.expires_at < ? OR job_leases.owner = ?",
        )
        .bind(job)
        .bind(owner)
        .bind(expires_at)
        .bind(now)
        .bind(owner)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub(super) async fn release(
        pool: &SqlitePool,
        job: &str,
        owner: &str,
    ) -> Result<(), LeaseError> {
        sqlx::query("DELETE FROM job_leases WHERE job_name = ? AND owner = ?")
            .bind(job)
            .bind(owner)
            .execute(pool)
            .await?;
        Ok(())
    }
}

mod postgres_impl {
    use super::{LeaseError, PgPool};

    pub(super) async fn try_acquire(
        pool: &PgPool,
        job: &str,
        owner: &str,
        now: i64,
        expires_at: i64,
    ) -> Result<bool, LeaseError> {
        let result = sqlx::query(
            "INSERT INTO job_leases (job_name, owner, expires_at) VALUES ($1, $2, $3) \
             ON CONFLICT (job_name) DO UPDATE \
             SET owner = EXCLUDED.owner, expires_at = EXCLUDED.expires_at \
             WHERE job_leases.expires_at < $4 OR job_leases.owner = $2",
        )
        .bind(job)
        .bind(owner)
        .bind(expires_at)
        .bind(now)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub(super) async fn release(pool: &PgPool, job: &str, owner: &str) -> Result<(), LeaseError> {
        sqlx::query("DELETE FROM job_leases WHERE job_name = $1 AND owner = $2")
            .bind(job)
            .bind(owner)
            .execute(pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
    use std::str::FromStr;

    const TTL: Duration = Duration::from_secs(60);

    /// Enough concurrent starts to make the window reliable rather than
    /// occasional. Two is the real-world minimum but races only sometimes.
    const INSTANCES: usize = 4;

    /// Creates the database the race below starts from, and returns the
    /// options every racing instance then connects with.
    ///
    /// Everything here is deliberately *outside* the race, and for one reason:
    /// each step is a write, and a write is a lock barrier that would serialise
    /// the instances for the wrong reason and prove nothing.
    ///
    /// - **WAL** is what makes the race reachable at all. Under the default
    ///   rollback journal a writer blocks readers, so a loser's "has this been
    ///   applied yet?" read cannot complete until the winner has committed, and
    ///   the check-then-act is serialised by accident. Under WAL readers
    ///   proceed against the pre-write snapshot, read a stale empty list, and
    ///   only then try to apply. Any deployment that sets WAL for concurrent
    ///   writers — which is the reason to run more than one process at all —
    ///   is in this case. (Switching journal mode also needs a brief exclusive
    ///   lock SQLite will not wait for, so racing the conversion itself would
    ///   test a different bug.)
    /// - **The bookkeeping table** is created up front because that is what
    ///   leaves the instances with nothing between them and the check-then-act.
    ///   It is also the real-world shape: a rolling upgrade meets a database
    ///   that has had that table since its first boot.
    /// - **`busy_timeout`** is set because this crate documents it as the
    ///   caller's job — it is what makes the loser of an ordinary write race
    ///   wait for the winner rather than fail immediately.
    async fn database_ready_to_be_upgraded(dir: &std::path::Path) -> SqliteConnectOptions {
        let url = format!("sqlite://{}?mode=rwc", dir.join("leases.db").display());
        let options = SqliteConnectOptions::from_str(&url)
            .expect("parse sqlite url")
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));

        let setup = SqlitePool::connect_with(options.clone())
            .await
            .expect("create the database in WAL mode");
        {
            let mut conn = setup.acquire().await.expect("setup connection");
            (*conn)
                .ensure_migrations_table(MIGRATIONS_TABLE)
                .await
                .expect("bookkeeping table");
        }
        setup.close().await;

        options
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn several_processes_can_create_the_table_at_once() {
        // This table is the one thing in a system built on these leases that
        // cannot itself be guarded by a lease, so its creation has to survive
        // the race on its own. Against `Migrator::run` this fails with
        // "table job_leases already exists".
        //
        // A pool each, not one shared pool, because that is what several
        // processes against one file actually look like -- a single pool would
        // serialise them for the wrong reason and prove nothing.
        let dir = tempfile::tempdir().expect("create temp dir");
        let options = database_ready_to_be_upgraded(dir.path()).await;

        // Separate tasks, not `tokio::join!`: joined futures share one task and
        // interleave only at await points, which is too gentle to expose this.
        // The barrier lines the migrations up so they start together.
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(INSTANCES));
        let mut instances = Vec::with_capacity(INSTANCES);
        for _ in 0..INSTANCES {
            let options = options.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            instances.push(tokio::spawn(async move {
                let pool = SqlitePool::connect_with(options).await.expect("pool");
                barrier.wait().await;
                SqlLease::sqlite(pool).await
            }));
        }
        for instance in instances {
            instance
                .await
                .expect("instance panicked")
                .expect("migrated");
        }

        let leases = SqlLease::sqlite(
            SqlitePool::connect_with(options)
                .await
                .expect("verifying pool"),
        )
        .await
        .expect("migrated");

        // And the table it produced is a working one, not just present.
        assert!(
            leases
                .try_acquire("sweep", "a", 1_000, TTL)
                .await
                .expect("acquire")
        );
    }

    /// One connection, so the in-memory database is shared across the pool
    /// rather than recreated per connection.
    async fn store() -> SqlLease {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        SqlLease::sqlite(pool).await.expect("migrate")
    }

    #[tokio::test]
    async fn an_unclaimed_job_can_be_claimed() {
        let leases = store().await;
        assert!(
            leases
                .try_acquire("sweep", "a", 1_000, TTL)
                .await
                .expect("acquire")
        );
    }

    #[tokio::test]
    async fn a_live_lease_blocks_another_owner() {
        let leases = store().await;
        leases
            .try_acquire("sweep", "a", 1_000, TTL)
            .await
            .expect("acquire");
        assert!(
            !leases
                .try_acquire("sweep", "b", 1_030, TTL)
                .await
                .expect("acquire"),
            "b claimed a lease a still holds"
        );
    }

    #[tokio::test]
    async fn an_expired_lease_does_not_block() {
        let leases = store().await;
        leases
            .try_acquire("sweep", "a", 1_000, TTL)
            .await
            .expect("acquire");
        assert!(
            leases
                .try_acquire("sweep", "b", 1_061, TTL)
                .await
                .expect("acquire"),
            "b could not claim a lease that expired at 1060"
        );
    }

    #[tokio::test]
    async fn the_holder_can_renew_before_expiry() {
        let leases = store().await;
        leases
            .try_acquire("sweep", "a", 1_000, TTL)
            .await
            .expect("acquire");
        assert!(
            leases
                .try_acquire("sweep", "a", 1_030, TTL)
                .await
                .expect("renew"),
            "the current holder could not renew"
        );
        // The renewal moved expiry to 1090, so a rival is still locked out at
        // 1061 -- where it would have succeeded without the renewal.
        assert!(
            !leases
                .try_acquire("sweep", "b", 1_061, TTL)
                .await
                .expect("acquire"),
            "the renewal did not extend the expiry"
        );
    }

    #[tokio::test]
    async fn releasing_lets_the_next_claimant_in_immediately() {
        let leases = store().await;
        leases
            .try_acquire("sweep", "a", 1_000, TTL)
            .await
            .expect("acquire");
        leases.release("sweep", "a").await.expect("release");
        assert!(
            leases
                .try_acquire("sweep", "b", 1_001, TTL)
                .await
                .expect("acquire"),
            "b could not claim a released lease"
        );
    }

    #[tokio::test]
    async fn releasing_someone_elses_lease_does_nothing() {
        let leases = store().await;
        leases
            .try_acquire("sweep", "a", 1_000, TTL)
            .await
            .expect("acquire");
        leases.release("sweep", "b").await.expect("release");
        assert!(
            !leases
                .try_acquire("sweep", "b", 1_001, TTL)
                .await
                .expect("acquire"),
            "b released a lease it did not hold"
        );
    }

    #[tokio::test]
    async fn distinct_jobs_do_not_contend() {
        let leases = store().await;
        leases
            .try_acquire("sweep", "a", 1_000, TTL)
            .await
            .expect("acquire");
        assert!(
            leases
                .try_acquire("reindex", "b", 1_000, TTL)
                .await
                .expect("acquire"),
            "an unrelated job was blocked"
        );
    }
}
