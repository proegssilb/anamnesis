//! [`SqlJobLease`]: [`anamnesis_app::JobLease`] backed by the `job-lease`
//! crate.
//!
//! Unlike its sibling modules, this one holds no SQL. `job-lease` is a
//! generic crate that knows nothing about anamnesis, so all this adapter does
//! is translate between the two vocabularies — [`Timestamp`] to `i64` unix
//! seconds on the way in, `job_lease::LeaseError` to
//! [`RepoError`](anamnesis_app::RepoError) on the way back. That translation
//! is the whole reason `anamnesis-app` can define a `JobLease` port without
//! acquiring a dependency on sqlx.
//!
//! The lease's table is migrated separately from everything else here:
//! `job-lease` tracks its own schema in `_job_lease_migrations` rather than
//! the `_sqlx_migrations` that [`SqlStore`]'s migrators use, so the two never
//! collide even when they share a database — as they do on Postgres. On
//! SQLite they do not share one at all; the leases get their own file, for
//! reasons `super::lease_database_url` sets out.

use std::time::Duration;

use anamnesis_app::{JobLease, RepoError};
use anamnesis_core::Timestamp;
use async_trait::async_trait;

use super::{Backend, SqlStore};

/// Implements [`anamnesis_app::JobLease`] over a `job_lease::SqlLease`.
#[derive(Debug, Clone)]
pub struct SqlJobLease {
    inner: job_lease::SqlLease,
}

impl SqlStore {
    /// Opens the job-lease store on the lease pool, creating its table if it
    /// is absent.
    ///
    /// Separate from [`SqlStore::connect`] because it is a separate schema
    /// with a separate migration history, and because a caller that never
    /// runs background work never needs it. Calling it is the first half of
    /// the two-step startup a lease implies: take the lease, then do the
    /// thing the lease guards. `SqlStore::connect` is itself such a caller —
    /// it takes the migration lease before running any migrations — which is
    /// why this cannot depend on those migrations having run.
    ///
    /// That makes the lease's own schema the base case, with no lease
    /// available to guard it. `job-lease` closes that itself, by running each
    /// of its migrations inside a `BEGIN IMMEDIATE` transaction.
    pub async fn job_lease(&self) -> Result<SqlJobLease, RepoError> {
        let inner = match &self.leases {
            Backend::Sqlite(pool) => job_lease::SqlLease::sqlite(pool.clone()).await,
            Backend::Postgres(pool) => job_lease::SqlLease::postgres(pool.clone()).await,
        }
        .map_err(|e| RepoError::from_source("failed to open the job-lease store", e))?;
        Ok(SqlJobLease { inner })
    }
}

#[async_trait]
impl JobLease for SqlJobLease {
    async fn try_acquire(
        &self,
        job: &str,
        owner: &str,
        now: Timestamp,
        ttl: Duration,
    ) -> Result<bool, RepoError> {
        self.inner
            .try_acquire(job, owner, now.unix_seconds(), ttl)
            .await
            .map_err(|e| RepoError::from_source(format!("failed to claim the {job:?} lease"), e))
    }

    async fn release(&self, job: &str, owner: &str) -> Result<(), RepoError> {
        self.inner
            .release(job, owner)
            .await
            .map_err(|e| RepoError::from_source(format!("failed to release the {job:?} lease"), e))
    }
}
