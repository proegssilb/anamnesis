//! [`AppSettings`]: the knobs `docs/DOMAIN.md` §3 assigns to a `Settings`
//! entity (`active_project_limit`, `timezone`, `sweep_recurrence`,
//! `suggestion config`) — stored as a real table in the schema
//! (`crates/anamnesis-adapters/migrations/*/0001_init.sql`'s `settings`
//! table) but read and written by no port in `anamnesis-app`: Phase D
//! defined no `SettingsRepository`, and the adapters' own module doc comment
//! flags exactly this ("no adapter code reads or writes it in this phase").
//!
//! Rather than block Phase F1's UI on a port this workspace has not yet
//! grown, these knobs are config-sourced constants for now — tunable at
//! deploy time via environment variables, not yet editable in the running
//! app (`ManageActiveProjectLimit`/`ManageSystemSettings` have no handler
//! yet). A future phase that adds `SettingsRepository` replaces this module
//! with a real read from the `settings` table.

use anamnesis_core::SuggestionSettings;

/// `docs/DOMAIN.md` §9 states cooldown length and sampling weights as open,
/// tune-later questions; three days is a reasonable starting point.
pub const DEFAULT_SUGGESTION_COOLDOWN_SECONDS: i64 = 3 * 24 * 60 * 60;

/// `bounce_count` at or above this earns the softer "this keeps coming back"
/// prompt (`docs/DOMAIN.md` §5).
pub const DEFAULT_HIGH_BOUNCE_THRESHOLD: u32 = 3;

/// How many projects may hold `status == Active` at once
/// (`docs/DOMAIN.md` §3's global invariant).
pub const DEFAULT_ACTIVE_PROJECT_LIMIT: u32 = 5;

#[derive(Debug, Clone)]
pub struct AppSettings {
    pub active_project_limit: u32,
    pub suggestion: SuggestionSettings,
    /// The IANA zone name every local-date calculation (the suggestion
    /// seed's `local_date`, a future sweep ticker) is resolved against —
    /// `ANAMNESIS_TIMEZONE`, validated once at startup (`main.rs`).
    pub timezone_name: String,
}

impl AppSettings {
    pub fn from_timezone(timezone_name: impl Into<String>) -> Self {
        AppSettings {
            active_project_limit: DEFAULT_ACTIVE_PROJECT_LIMIT,
            suggestion: SuggestionSettings {
                cooldown_seconds: DEFAULT_SUGGESTION_COOLDOWN_SECONDS,
                high_bounce_threshold: DEFAULT_HIGH_BOUNCE_THRESHOLD,
            },
            timezone_name: timezone_name.into(),
        }
    }
}
