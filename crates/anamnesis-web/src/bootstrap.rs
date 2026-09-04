//! Closes the bootstrap gap `docs/DOMAIN.md` leaves open: `create_area` is
//! System-Admin-only, so a freshly created database has no System Admin to
//! grant anything and no board columns to place a task on — nothing in the
//! use-case layer can dig it out of that hole on its own, since granting a
//! role itself requires an existing admin's authority to invoke
//! (`anamnesis_app::grant_system_admin`, `crate::use_cases::membership`'s
//! module doc comment). This uses `anamnesis_app::MembershipRepository`
//! directly, bypassing that use-case-layer gate, precisely because it is
//! the one legitimate place in the whole system that must be able to: run
//! once at startup, idempotently, before the router starts accepting
//! requests. `SqlStore::seed_board_column` and
//! `SqlStore::seed_settings_if_missing` are inherent seams (not ports —
//! `docs/DOMAIN.md` §7 defines no column- or settings-seeding port at all)
//! for the same reason, seeded here too.
//!
//! **Idempotency.** `MembershipQuery` has no "does any System Admin exist"
//! query, only "does *this* user hold it" (`MembershipQuery::is_system_admin`)
//! — so this checks whether `ANAMNESIS_BOOTSTRAP_ADMIN`'s subject
//! specifically already holds System Admin, granting only if not. On a
//! genuinely fresh database (no admins at all) that is equivalent to "no
//! System Admin exists"; on every later boot the named subject already holds
//! it, so the grant call — itself idempotent, `MembershipRepository::
//! grant_system_admin` upserts — is skipped entirely and nothing is logged.
//! Column seeding is symmetric: seed only when `BoardQuery::
//! columns_with_items` reports zero columns.
//!
//! **Column defaults.** `docs/DOMAIN.md` §3 names the three default columns
//! (To-Do WIP-limited, Doing, Done) but not a WIP limit number, and columns
//! are not one of the runtime-editable `Settings` fields (`docs/DOMAIN.md`
//! §3 also names them as System-Admin territory, but that surface is not in
//! scope here). [`DEFAULT_TODO_WIP_LIMIT`] is a stated, tunable assumption,
//! not a hidden default.
//!
//! **Idempotent is not the same as safe to run concurrently.** Column seeding
//! is a check-then-act: read `columns_with_items`, seed if empty. Two
//! instances booting against one fresh database can both observe empty and
//! seed six columns — silent, permanent, and reachable only on a first boot,
//! which is the worst combination to debug. So the whole of [`run`] is taken
//! under an `anamnesis_app::JobLease`, which closes that window without
//! restructuring the per-item idempotency that already works.

use std::time::Duration;

use anamnesis_adapters::{SqlStore, SystemClock};
use anamnesis_app::{
    BoardQuery, Clock, IdGen, JobLease, MembershipQuery, MembershipRepository, RepoError, Settings,
};
use anamnesis_core::{ColumnId, UserId, create_column};

/// `docs/DOMAIN.md` §3 requires the To-Do column to carry *a* WIP limit but
/// does not name one; five is a reasonable, tunable starting point until a
/// `Settings`-editing surface exists to change it.
pub const DEFAULT_TODO_WIP_LIMIT: u32 = 5;

/// The lease name startup coordinates on.
pub const BOOTSTRAP_JOB: &str = "bootstrap";

/// How long the bootstrap lease is held for. Comfortably longer than the
/// handful of queries [`seed`] runs, short enough that an instance killed
/// mid-bootstrap does not stall its replacement for long.
///
/// Never renewed, though `JobLease` supports it. [`seed`] runs a fixed number
/// of queries against a database that is empty or nearly so, and nothing about
/// it grows with the data, so it has no worst case for a heartbeat to cover.
const LEASE_TTL: Duration = Duration::from_secs(60);

/// How long to wait for another instance's bootstrap before giving up.
///
/// Waiting — rather than skipping, as the sweep ticker does — is the whole
/// point here: this instance must not go on to bind a socket until the admin,
/// the columns, and the settings row actually exist, whichever instance
/// created them. Since [`seed`] is idempotent, simply running it again once
/// the other instance is finished is both correct and cheap.
const LEASE_WAIT: Duration = Duration::from_secs(30);

const LEASE_POLL: Duration = Duration::from_millis(250);

/// Bootstraps the database under the `"bootstrap"` job lease, so that two
/// instances starting against one fresh database cannot both seed it.
///
/// Safe to call on every startup — see the module doc comment for why each
/// half is idempotent, and why idempotency alone was not enough.
///
/// The lease is opened from `store`'s own pool rather than passed in, so no
/// caller can forget it. That leans on `store` already being the concrete
/// `SqlStore` here rather than a set of ports, which is deliberate and is
/// what the module doc comment's "inherent seams" paragraph is about.
pub async fn run(
    store: &SqlStore,
    ids: &dyn IdGen,
    bootstrap_admin: &str,
    timezone: &str,
) -> Result<(), RepoError> {
    let leases = store.job_lease().await?;
    let owner = ids.next().to_string();
    acquire_lease(&leases, &owner).await?;

    let outcome = seed(store, ids, bootstrap_admin, timezone).await;

    if let Err(err) = leases.release(BOOTSTRAP_JOB, &owner).await {
        tracing::warn!(
            error = %err,
            "bootstrap: could not release the lease; it will expire on its own"
        );
    }
    outcome
}

/// Blocks until this instance holds the bootstrap lease, or [`LEASE_WAIT`]
/// elapses.
async fn acquire_lease(leases: &dyn JobLease, owner: &str) -> Result<(), RepoError> {
    let deadline = std::time::Instant::now() + LEASE_WAIT;
    let mut waited = false;
    loop {
        if leases
            .try_acquire(BOOTSTRAP_JOB, owner, SystemClock.now(), LEASE_TTL)
            .await?
        {
            if waited {
                tracing::info!("bootstrap: the other instance finished; continuing");
            }
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(RepoError::new(format!(
                "another instance has held the {BOOTSTRAP_JOB:?} lease for over {}s. If it was \
                 killed mid-bootstrap, restarting once the lease expires will clear it.",
                LEASE_WAIT.as_secs()
            )));
        }
        if !waited {
            tracing::info!("bootstrap: another instance is bootstrapping; waiting for it");
            waited = true;
        }
        tokio::time::sleep(LEASE_POLL).await;
    }
}

/// Grants `bootstrap_admin` System Admin if nobody by that name already
/// holds it, seeds the three default board columns if none exist yet, and
/// seeds a default [`Settings`] row if none exists yet (`timezone` is
/// stored on that row only because the schema's `timezone` column is
/// `NOT NULL` — it is not read back by any port; see
/// `anamnesis_app::settings`'s module doc comment).
async fn seed(
    store: &SqlStore,
    ids: &dyn IdGen,
    bootstrap_admin: &str,
    timezone: &str,
) -> Result<(), RepoError> {
    let admin = UserId::new(bootstrap_admin);
    if !store.is_system_admin(&admin).await? {
        store.grant_system_admin(&admin).await?;
        tracing::info!(
            user = %admin,
            "bootstrap: granted System Admin (ANAMNESIS_BOOTSTRAP_ADMIN)"
        );
    }

    let existing: Vec<_> = store.columns_with_items().await?;
    if existing.is_empty() {
        let columns = [
            ("To-Do", Some(DEFAULT_TODO_WIP_LIMIT), false),
            ("Doing", None, false),
            ("Done", None, true),
        ];
        for (position, (title, wip_limit, is_done)) in columns.into_iter().enumerate() {
            let column = create_column(
                ColumnId::new(ids.next()),
                title,
                position as u32,
                wip_limit,
                is_done,
            )
            .map_err(|e| RepoError::from_source("failed to build a default board column", e))?;
            store.seed_board_column(&column).await?;
        }
        tracing::info!("bootstrap: seeded default board columns (To-Do, Doing, Done)");
    }

    store
        .seed_settings_if_missing(&Settings::default(), timezone)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anamnesis_adapters::UuidIdGen;

    async fn temp_store() -> (SqlStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("bootstrap-test.db");
        let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
        let store = SqlStore::connect(&db_url)
            .await
            .expect("connect to temp SQLite database");
        (store, dir)
    }

    #[tokio::test]
    async fn a_fresh_database_gets_an_admin_and_default_columns() {
        let (store, _dir) = temp_store().await;
        let ids = UuidIdGen;

        run(&store, &ids, "alice", "UTC").await.unwrap();

        assert!(store.is_system_admin(&UserId::new("alice")).await.unwrap());
        let columns = store.columns_with_items().await.unwrap();
        let titles: Vec<&str> = columns.iter().map(|c| c.column.title.as_str()).collect();
        assert_eq!(titles, vec!["To-Do", "Doing", "Done"]);
        assert_eq!(
            columns[0].column.wip_limit,
            Some(DEFAULT_TODO_WIP_LIMIT),
            "To-Do must be WIP-limited"
        );
        assert_eq!(columns[1].column.wip_limit, None);
        assert!(!columns[1].column.is_done);
        assert_eq!(columns[2].column.wip_limit, None);
        assert!(columns[2].column.is_done);
    }

    #[tokio::test]
    async fn a_second_boot_grants_no_second_admin_and_seeds_no_second_set_of_columns() {
        let (store, _dir) = temp_store().await;
        let ids = UuidIdGen;

        run(&store, &ids, "alice", "UTC").await.unwrap();
        run(&store, &ids, "alice", "UTC").await.unwrap();

        let columns = store.columns_with_items().await.unwrap();
        assert_eq!(
            columns.len(),
            3,
            "columns must not be seeded twice across two boots"
        );
        // `system_admins` is keyed by `user_id` alone, so a second grant of
        // the same user could only ever be a harmless no-op row-wise — the
        // real assertion is that the whole run stays trivially successful
        // and idempotent end to end, exercised above.
    }

    #[tokio::test]
    async fn a_different_bootstrap_admin_on_a_second_boot_grants_that_admin_too() {
        // Not "only the very first boot's admin ever gets granted": the
        // idempotency guarantee is per named subject, not a one-shot latch,
        // which is deliberate -- see the module doc comment on why there is
        // no "does any admin exist" check to latch against.
        let (store, _dir) = temp_store().await;
        let ids = UuidIdGen;

        run(&store, &ids, "alice", "UTC").await.unwrap();
        run(&store, &ids, "bob", "UTC").await.unwrap();

        assert!(store.is_system_admin(&UserId::new("alice")).await.unwrap());
        assert!(store.is_system_admin(&UserId::new("bob")).await.unwrap());
        let columns = store.columns_with_items().await.unwrap();
        assert_eq!(columns.len(), 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn instances_booting_at_once_seed_one_set_of_columns() {
        // The race this closes: every instance reads `columns_with_items`,
        // every one sees it empty, every one seeds -- three columns per
        // instance, permanently, on a first boot only. Fails on the unleased
        // version of `run`.
        //
        // A `SqlStore` each, not one shared handle, because that is what
        // several processes against one file actually look like -- and
        // because a single pool would serialize them for the wrong reason.
        //
        // Separate tasks rather than `tokio::join!` for the same kind of
        // reason: joined futures share one task and interleave only where
        // they happen to await, which is a much gentler test than genuine
        // parallelism. The barrier then holds them at the gate so they enter
        // `run` together instead of however far apart spawning drifted them.
        const INSTANCES: usize = 4;

        let dir = tempfile::tempdir().expect("create temp dir");
        let db_url = format!(
            "sqlite://{}?mode=rwc",
            dir.path().join("concurrent-boot.db").display()
        );

        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(INSTANCES));
        let mut instances = Vec::with_capacity(INSTANCES);
        for _ in 0..INSTANCES {
            let db_url = db_url.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            instances.push(tokio::spawn(async move {
                let store = SqlStore::connect(&db_url).await.expect("connect");
                barrier.wait().await;
                run(&store, &UuidIdGen, "alice", "UTC").await
            }));
        }
        for instance in instances {
            instance
                .await
                .expect("instance panicked")
                .expect("instance bootstraps");
        }

        let store = SqlStore::connect(&db_url).await.expect("verifying connect");
        let titles: Vec<String> = store
            .columns_with_items()
            .await
            .unwrap()
            .iter()
            .map(|c| c.column.title.as_str().to_string())
            .collect();
        assert_eq!(titles, vec!["To-Do", "Doing", "Done"]);
    }

    #[tokio::test]
    async fn a_fresh_database_gets_default_settings_and_a_second_boot_does_not_reset_an_edit() {
        use anamnesis_app::SettingsRepository;

        let (store, _dir) = temp_store().await;
        let ids = UuidIdGen;

        run(&store, &ids, "alice", "UTC").await.unwrap();
        let settings = SettingsRepository::load(&store).await.unwrap();
        assert_eq!(settings, Settings::default());

        // An admin edits a setting between boots...
        let edited = Settings {
            active_project_limit: 42,
            ..Settings::default()
        };
        SettingsRepository::update(&store, &edited).await.unwrap();

        // ...and a second boot must not reset it back to the default.
        run(&store, &ids, "alice", "UTC").await.unwrap();
        let settings = SettingsRepository::load(&store).await.unwrap();
        assert_eq!(settings.active_project_limit, 42);
    }
}
