# job-lease

Expiry-based leases in an ordinary SQL table, so that exactly one process out
of several runs a given named job. Works on both SQLite and Postgres.

## The problem

You have a web app that runs some recurring work on a timer — an archive
sweep, a reindex, a garbage collection — and you want to run more than one
instance of it for availability or zero-downtime upgrades. Suddenly every
instance fires the timer, and the work happens N times.

Existing crates do not fit that shape well:

- **Job queues** (`apalis`, `sqlxmq`, `underway`) solve scheduling and retries,
  which is more machinery than mutual exclusion needs — and they only relocate
  the problem, since something must still decide which instance *enqueues* each
  scheduled tick. Two of the three are Postgres-only.
- **`kube-lease-manager`** does leader election properly, but wants a
  Kubernetes cluster.
- **Consensus crates** (`openraft`, `hiqlite`) are the right answer when there
  is no shared store, and redundant when there already is one.
- **`lease`**, despite the name, is an in-process object pool.

If your instances already share a database, that database is a perfectly good
coordinator. This crate is the ~200 lines that make it one.

## Usage

```rust
use std::time::Duration;
use job_lease::SqlLease;

let pool = sqlx::SqlitePool::connect("sqlite://app.db").await?;
let leases = SqlLease::sqlite(pool).await?;

let now = 1_760_000_000; // unix seconds, from whatever clock you trust
if leases.try_acquire("archive_sweep", "instance-a", now, Duration::from_secs(300)).await? {
    // ... do the work; no other instance is doing it ...
    leases.release("archive_sweep", "instance-a").await?;
}
```

`SqlLease::postgres` is the same, for a `PgPool`.

The constructor runs the crate's own migration to create a `job_leases` table.
It tracks that in `_job_lease_migrations`, not the default `_sqlx_migrations`,
so it will not collide with your application's own migrations. Startup is
strictly two-step: take the lease, then do the thing the lease guards.

`now` is a parameter rather than something the crate reads, so that a caller
with a testable clock abstraction can keep using it, and so tests can advance
time without sleeping.

A `JobLease` trait is exported for substituting a fake in your own tests, but
using it is optional — the inherent methods on `SqlLease` have the same
signatures.

## Which topologies this works for

Anything where every instance can see the same rows:

- Postgres, any number of machines.
- SQLite, any number of processes **on one machine**, sharing the file.

It does **not** work for replicated SQLite where each instance has its own
file, because a lease row only coordinates the instances that can see it — each
replica would win its own private lease. Use a consensus-based tool for that
topology.

## You build the pool

This crate borrows a pool rather than taking a URL. Pool configuration is a
deployment concern, and a library that took it over would be making your
decisions for you.

That leaves one precondition you must satisfy on SQLite: **set `busy_timeout`**
if more than one process will use the database. sqlx's SQLite migrator does no
locking of its own — its `lock` and `unlock` are empty — so on a first
concurrent start, SQLite's own busy handling is the only thing serialising two
processes creating the table. WAL journal mode is also worth setting for
concurrent writers.

```rust
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

let options = SqliteConnectOptions::new()
    .filename("app.db")
    .create_if_missing(true)
    .journal_mode(SqliteJournalMode::Wal)
    .busy_timeout(Duration::from_secs(5));
let pool = SqlitePoolOptions::new().connect_with(options).await?;
```

Postgres needs no equivalent care: sqlx's Postgres migrator holds an advisory
lock, keyed on the database name, for the whole run.

## Why expiry and not a held lock

A lease expires on its own, so a holder that crashes mid-job releases it by
doing nothing. The alternative — a connection-scoped lock such as Postgres'
`pg_advisory_lock` — leaks when a pooled connection is returned or when a
future is cancelled at a `tokio::time::timeout`, and cannot be released in
`Drop` because releasing requires an `await`.

The cost is that a lease is only as good as the clocks involved. Pick a TTL
comfortably longer than the job's worst-case runtime, and treat the job itself
as idempotent anyway. This is coordination that saves you N× duplicated work,
not a distributed mutex you can bet correctness on.

## License

BSD-3-Clause.
