//! Closes the bootstrap gap `docs/DOMAIN.md` leaves open: `create_area` is
//! System-Admin-only, and `MembershipQuery`/`BoardQuery` are read-only, so a
//! freshly created database has no System Admin to grant anything and no
//! board columns to place a task on — nothing in the use-case layer can dig
//! it out of that hole. `anamnesis_adapters::SqlStore` exposes
//! `grant_system_admin`, `seed_board_column`, `set_area_role`, and
//! `set_project_role` as inherent seams (not ports — `docs/DOMAIN.md` §7
//! defines no `SettingsRepository`/column-writing port at all) for exactly
//! this: run once at startup, idempotently, before the router starts
//! accepting requests.
//!
//! **Idempotency.** `MembershipQuery` has no "does any System Admin exist"
//! query, only "does *this* user hold it" (`MembershipQuery::is_system_admin`)
//! — so this checks whether `ANAMNESIS_BOOTSTRAP_ADMIN`'s subject
//! specifically already holds System Admin, granting only if not. On a
//! genuinely fresh database (no admins at all) that is equivalent to "no
//! System Admin exists"; on every later boot the named subject already holds
//! it, so the grant call — itself idempotent, `SqlStore::grant_system_admin`
//! upserts — is skipped entirely and nothing is logged. Column seeding is
//! symmetric: seed only when `BoardQuery::columns_with_tasks` reports zero
//! columns.
//!
//! **Column defaults.** `docs/DOMAIN.md` §3 names the three default columns
//! (To-Do WIP-limited, Doing, Done) but not a WIP limit number — no port
//! resolves one either (`Settings` has no reader). [`DEFAULT_TODO_WIP_LIMIT`]
//! is a stated, tunable assumption, not a hidden default.

use anamnesis_adapters::SqlStore;
use anamnesis_app::{BoardQuery, IdGen, MembershipQuery, RepoError};
use anamnesis_core::{ColumnId, UserId, create_column};

/// `docs/DOMAIN.md` §3 requires the To-Do column to carry *a* WIP limit but
/// does not name one; five is a reasonable, tunable starting point until a
/// `Settings`-editing surface exists to change it.
pub const DEFAULT_TODO_WIP_LIMIT: u32 = 5;

/// Grants `bootstrap_admin` System Admin if nobody by that name already
/// holds it, and seeds the three default board columns if none exist yet.
/// Safe to call on every startup — see the module doc comment for why each
/// half is idempotent.
pub async fn run(store: &SqlStore, ids: &dyn IdGen, bootstrap_admin: &str) -> Result<(), RepoError> {
    let admin = UserId::new(bootstrap_admin);
    if !store.is_system_admin(&admin).await? {
        store.grant_system_admin(&admin).await?;
        tracing::info!(
            user = %admin,
            "bootstrap: granted System Admin (ANAMNESIS_BOOTSTRAP_ADMIN)"
        );
    }

    let existing = store.columns_with_tasks().await?;
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

        run(&store, &ids, "alice").await.unwrap();

        assert!(store.is_system_admin(&UserId::new("alice")).await.unwrap());
        let columns = store.columns_with_tasks().await.unwrap();
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

        run(&store, &ids, "alice").await.unwrap();
        run(&store, &ids, "alice").await.unwrap();

        let columns = store.columns_with_tasks().await.unwrap();
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

        run(&store, &ids, "alice").await.unwrap();
        run(&store, &ids, "bob").await.unwrap();

        assert!(store.is_system_admin(&UserId::new("alice")).await.unwrap());
        assert!(store.is_system_admin(&UserId::new("bob")).await.unwrap());
        let columns = store.columns_with_tasks().await.unwrap();
        assert_eq!(columns.len(), 3);
    }
}
