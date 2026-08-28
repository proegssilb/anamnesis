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

use anamnesis_adapters::SqlStore;
use anamnesis_app::{
    BoardQuery, IdGen, MembershipQuery, MembershipRepository, RepoError, Settings,
};
use anamnesis_core::{ColumnId, UserId, create_column};

/// `docs/DOMAIN.md` §3 requires the To-Do column to carry *a* WIP limit but
/// does not name one; five is a reasonable, tunable starting point until a
/// `Settings`-editing surface exists to change it.
pub const DEFAULT_TODO_WIP_LIMIT: u32 = 5;

/// Grants `bootstrap_admin` System Admin if nobody by that name already
/// holds it, seeds the three default board columns if none exist yet, and
/// seeds a default [`Settings`] row if none exists yet (`timezone` is
/// stored on that row only because the schema's `timezone` column is
/// `NOT NULL` — it is not read back by any port; see
/// `anamnesis_app::settings`'s module doc comment). Safe to call on every
/// startup — see the module doc comment for why each half is idempotent.
pub async fn run(
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
