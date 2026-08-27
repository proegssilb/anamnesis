//! [`SettingsRepository`] over [`SqlStore`]: the singleton `settings` row.
//! `id = 1` is enforced by the schema's own `CHECK` constraint, so every
//! query here is unconditionally scoped to that one row.
//!
//! `SqlStore::seed_settings_if_missing` (not a port method — an inherent
//! seam, exactly like `SqlStore::seed_board_column` and
//! `SqlStore::grant_system_admin`, which `anamnesis-web::bootstrap`'s own
//! doc comment explains the pattern for) is what actually creates the row
//! on first boot; every method below assumes it already exists, matching
//! [`anamnesis_app::SettingsRepository::load`]'s own doc comment.

use anamnesis_app::{RepoError, Settings, SettingsRepository};
use anamnesis_core::{Recurrence, SuggestionSettings, Timestamp};
use async_trait::async_trait;
use sqlx::{PgPool, Row, SqlitePool};

use super::{Backend, SqlStore, timestamp_from_seconds, weekday_from_text, weekday_to_text};

/// The stored column values a [`Recurrence`] encodes to (and decodes from):
/// `(kind, n, weekday, day)`.
fn encode_recurrence(
    recurrence: Recurrence,
) -> (&'static str, Option<i64>, Option<&'static str>, Option<i64>) {
    match recurrence {
        Recurrence::Never => ("never", None, None, None),
        Recurrence::EveryNWeeks { n, weekday } => (
            "every_n_weeks",
            Some(i64::from(n)),
            Some(weekday_to_text(weekday)),
            None,
        ),
        Recurrence::DayOfMonth { day } => ("day_of_month", None, None, Some(i64::from(day))),
    }
}

fn decode_recurrence(
    kind: &str,
    n: Option<i64>,
    weekday: Option<String>,
    day: Option<i64>,
) -> Result<Recurrence, RepoError> {
    match kind {
        "never" => Ok(Recurrence::Never),
        "every_n_weeks" => {
            let n = n.ok_or_else(|| RepoError::new("stored every_n_weeks recurrence missing n"))?;
            let n = u8::try_from(n)
                .map_err(|e| RepoError::from_source("invalid stored recurrence n", e))?;
            let weekday = weekday_from_text(weekday.as_deref().ok_or_else(|| {
                RepoError::new("stored every_n_weeks recurrence missing weekday")
            })?)?;
            Ok(Recurrence::EveryNWeeks { n, weekday })
        }
        "day_of_month" => {
            let day =
                day.ok_or_else(|| RepoError::new("stored day_of_month recurrence missing day"))?;
            let day = u8::try_from(day)
                .map_err(|e| RepoError::from_source("invalid stored recurrence day", e))?;
            Ok(Recurrence::DayOfMonth { day })
        }
        other => Err(RepoError::new(format!(
            "invalid stored sweep_recurrence_kind {other:?}"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
fn assemble(
    active_project_limit: i64,
    sweep_recurrence_kind: String,
    sweep_recurrence_n: Option<i64>,
    sweep_recurrence_weekday: Option<String>,
    sweep_recurrence_day: Option<i64>,
    suggestion_cooldown_seconds: i64,
    suggestion_high_bounce_threshold: i64,
    last_swept_at: Option<i64>,
) -> Result<Settings, RepoError> {
    Ok(Settings {
        active_project_limit: u32::try_from(active_project_limit)
            .map_err(|e| RepoError::from_source("invalid stored active_project_limit", e))?,
        suggestion: SuggestionSettings {
            cooldown_seconds: suggestion_cooldown_seconds,
            high_bounce_threshold: u32::try_from(suggestion_high_bounce_threshold)
                .map_err(|e| RepoError::from_source("invalid stored high_bounce_threshold", e))?,
        },
        sweep_recurrence: decode_recurrence(
            &sweep_recurrence_kind,
            sweep_recurrence_n,
            sweep_recurrence_weekday,
            sweep_recurrence_day,
        )?,
        last_swept_at: last_swept_at.map(timestamp_from_seconds).transpose()?,
    })
}

impl SqlStore {
    /// Inserts the singleton settings row with `defaults`'s values (and
    /// `timezone` for the schema's `NOT NULL timezone` column — a leftover
    /// no port reads, see `crate::sql::settings`'s module doc comment) if
    /// no row exists yet. A single atomic "insert, ignore on conflict"
    /// statement rather than a check-then-insert: idempotent by
    /// construction, safe to call on every startup exactly like
    /// `SqlStore::seed_board_column`.
    pub async fn seed_settings_if_missing(
        &self,
        defaults: &Settings,
        timezone: &str,
    ) -> Result<(), RepoError> {
        let (kind, n, weekday, day) = encode_recurrence(defaults.sweep_recurrence);
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlx::query(
                    "INSERT OR IGNORE INTO settings \
                     (id, active_project_limit, timezone, sweep_recurrence_kind, \
                      sweep_recurrence_n, sweep_recurrence_weekday, sweep_recurrence_day, \
                      suggestion_cooldown_seconds, suggestion_high_bounce_threshold) \
                     VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(i64::from(defaults.active_project_limit))
                .bind(timezone)
                .bind(kind)
                .bind(n)
                .bind(weekday)
                .bind(day)
                .bind(defaults.suggestion.cooldown_seconds)
                .bind(i64::from(defaults.suggestion.high_bounce_threshold))
                .execute(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to seed default settings", e))?;
            }
            Backend::Postgres(pool) => {
                let active_project_limit = i32::try_from(defaults.active_project_limit)
                    .map_err(|e| RepoError::from_source("active_project_limit out of range", e))?;
                let high_bounce_threshold =
                    i32::try_from(defaults.suggestion.high_bounce_threshold).map_err(|e| {
                        RepoError::from_source("high_bounce_threshold out of range", e)
                    })?;
                let cooldown_seconds = i32::try_from(defaults.suggestion.cooldown_seconds)
                    .map_err(|e| RepoError::from_source("cooldown_seconds out of range", e))?;
                let n = n
                    .map(i32::try_from)
                    .transpose()
                    .map_err(|e| RepoError::from_source("recurrence n out of range", e))?;
                let day = day
                    .map(i32::try_from)
                    .transpose()
                    .map_err(|e| RepoError::from_source("recurrence day out of range", e))?;
                sqlx::query(
                    "INSERT INTO settings \
                     (id, active_project_limit, timezone, sweep_recurrence_kind, \
                      sweep_recurrence_n, sweep_recurrence_weekday, sweep_recurrence_day, \
                      suggestion_cooldown_seconds, suggestion_high_bounce_threshold) \
                     VALUES (1, $1, $2, $3, $4, $5, $6, $7, $8) \
                     ON CONFLICT (id) DO NOTHING",
                )
                .bind(active_project_limit)
                .bind(timezone)
                .bind(kind)
                .bind(n)
                .bind(weekday)
                .bind(day)
                .bind(cooldown_seconds)
                .bind(high_bounce_threshold)
                .execute(pool)
                .await
                .map_err(|e| RepoError::from_source("failed to seed default settings", e))?;
            }
        }
        Ok(())
    }
}

mod sqlite_impl {
    use super::*;

    pub(super) async fn load(pool: &SqlitePool) -> Result<Settings, RepoError> {
        let row = sqlx::query(
            "SELECT active_project_limit, sweep_recurrence_kind, sweep_recurrence_n, \
             sweep_recurrence_weekday, sweep_recurrence_day, suggestion_cooldown_seconds, \
             suggestion_high_bounce_threshold, last_swept_at FROM settings WHERE id = 1",
        )
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load settings", e))?
        .ok_or_else(|| RepoError::new("no settings row -- bootstrap did not seed one"))?;
        assemble(
            row.get::<i64, _>("active_project_limit"),
            row.get("sweep_recurrence_kind"),
            row.get::<Option<i64>, _>("sweep_recurrence_n"),
            row.get::<Option<String>, _>("sweep_recurrence_weekday"),
            row.get::<Option<i64>, _>("sweep_recurrence_day"),
            row.get::<i64, _>("suggestion_cooldown_seconds"),
            row.get::<i64, _>("suggestion_high_bounce_threshold"),
            row.get::<Option<i64>, _>("last_swept_at"),
        )
    }

    pub(super) async fn update(pool: &SqlitePool, settings: &Settings) -> Result<(), RepoError> {
        let (kind, n, weekday, day) = encode_recurrence(settings.sweep_recurrence);
        sqlx::query(
            "UPDATE settings SET active_project_limit = ?, sweep_recurrence_kind = ?, \
             sweep_recurrence_n = ?, sweep_recurrence_weekday = ?, sweep_recurrence_day = ?, \
             suggestion_cooldown_seconds = ?, suggestion_high_bounce_threshold = ? WHERE id = 1",
        )
        .bind(i64::from(settings.active_project_limit))
        .bind(kind)
        .bind(n)
        .bind(weekday)
        .bind(day)
        .bind(settings.suggestion.cooldown_seconds)
        .bind(i64::from(settings.suggestion.high_bounce_threshold))
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to update settings", e))?;
        Ok(())
    }

    pub(super) async fn record_sweep(
        pool: &SqlitePool,
        swept_at: Timestamp,
    ) -> Result<(), RepoError> {
        sqlx::query("UPDATE settings SET last_swept_at = ? WHERE id = 1")
            .bind(swept_at.unix_seconds())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to record a sweep", e))?;
        Ok(())
    }
}

mod postgres_impl {
    use super::*;

    pub(super) async fn load(pool: &PgPool) -> Result<Settings, RepoError> {
        let row = sqlx::query(
            "SELECT active_project_limit, sweep_recurrence_kind, sweep_recurrence_n, \
             sweep_recurrence_weekday, sweep_recurrence_day, suggestion_cooldown_seconds, \
             suggestion_high_bounce_threshold, last_swept_at FROM settings WHERE id = 1",
        )
        .fetch_optional(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to load settings", e))?
        .ok_or_else(|| RepoError::new("no settings row -- bootstrap did not seed one"))?;
        assemble(
            i64::from(row.get::<i32, _>("active_project_limit")),
            row.get("sweep_recurrence_kind"),
            row.get::<Option<i32>, _>("sweep_recurrence_n")
                .map(i64::from),
            row.get::<Option<String>, _>("sweep_recurrence_weekday"),
            row.get::<Option<i32>, _>("sweep_recurrence_day")
                .map(i64::from),
            i64::from(row.get::<i32, _>("suggestion_cooldown_seconds")),
            i64::from(row.get::<i32, _>("suggestion_high_bounce_threshold")),
            row.get::<Option<i64>, _>("last_swept_at"),
        )
    }

    pub(super) async fn update(pool: &PgPool, settings: &Settings) -> Result<(), RepoError> {
        let (kind, n, weekday, day) = encode_recurrence(settings.sweep_recurrence);
        let active_project_limit = i32::try_from(settings.active_project_limit)
            .map_err(|e| RepoError::from_source("active_project_limit out of range", e))?;
        let high_bounce_threshold = i32::try_from(settings.suggestion.high_bounce_threshold)
            .map_err(|e| RepoError::from_source("high_bounce_threshold out of range", e))?;
        let cooldown_seconds = i32::try_from(settings.suggestion.cooldown_seconds)
            .map_err(|e| RepoError::from_source("cooldown_seconds out of range", e))?;
        let n = n
            .map(i32::try_from)
            .transpose()
            .map_err(|e| RepoError::from_source("recurrence n out of range", e))?;
        let day = day
            .map(i32::try_from)
            .transpose()
            .map_err(|e| RepoError::from_source("recurrence day out of range", e))?;
        sqlx::query(
            "UPDATE settings SET active_project_limit = $1, sweep_recurrence_kind = $2, \
             sweep_recurrence_n = $3, sweep_recurrence_weekday = $4, sweep_recurrence_day = $5, \
             suggestion_cooldown_seconds = $6, suggestion_high_bounce_threshold = $7 WHERE id = 1",
        )
        .bind(active_project_limit)
        .bind(kind)
        .bind(n)
        .bind(weekday)
        .bind(day)
        .bind(cooldown_seconds)
        .bind(high_bounce_threshold)
        .execute(pool)
        .await
        .map_err(|e| RepoError::from_source("failed to update settings", e))?;
        Ok(())
    }

    pub(super) async fn record_sweep(pool: &PgPool, swept_at: Timestamp) -> Result<(), RepoError> {
        sqlx::query("UPDATE settings SET last_swept_at = $1 WHERE id = 1")
            .bind(swept_at.unix_seconds())
            .execute(pool)
            .await
            .map_err(|e| RepoError::from_source("failed to record a sweep", e))?;
        Ok(())
    }
}

#[async_trait]
impl SettingsRepository for SqlStore {
    async fn load(&self) -> Result<Settings, RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::load(pool).await,
            Backend::Postgres(pool) => postgres_impl::load(pool).await,
        }
    }

    async fn update(&self, settings: &Settings) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::update(pool, settings).await,
            Backend::Postgres(pool) => postgres_impl::update(pool, settings).await,
        }
    }

    async fn record_sweep(&self, swept_at: Timestamp) -> Result<(), RepoError> {
        match &self.backend {
            Backend::Sqlite(pool) => sqlite_impl::record_sweep(pool, swept_at).await,
            Backend::Postgres(pool) => postgres_impl::record_sweep(pool, swept_at).await,
        }
    }
}
