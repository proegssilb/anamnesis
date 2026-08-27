//! [`Settings`]: the runtime-editable knobs `docs/DOMAIN.md` §3 assigns to a
//! `Settings` entity, and the port that loads/writes them
//! ([`crate::ports::SettingsRepository`]).
//!
//! **Timezone is deliberately not one of these fields.** `docs/DOMAIN.md`
//! §3 lists `timezone` on the `Settings` entity, but the schema's `settings`
//! table already carries a `timezone` column that predates this port and
//! nothing here reads or writes it: `ANAMNESIS_TIMEZONE` plus
//! `crate::ports::TimezoneResolver` remain the source of truth for every
//! local-date calculation (the suggestion seed's `local_date`, the sweep
//! ticker), exactly as they were before this port existed. Folding timezone
//! into this port too would mean either a second, redundant place to change
//! it or a startup-time sync between the two — neither buys anything a
//! config variable was not already doing correctly, and the column stays in
//! the schema unread by this port rather than being removed, so a future
//! phase that *does* want it runtime-editable has somewhere to put it.
//!
//! **`last_swept_at` gets its own targeted write
//! ([`crate::ports::SettingsRepository::record_sweep`]), not the general
//! [`crate::ports::SettingsRepository::update`].** The sweep ticker
//! (`anamnesis-web`) and an admin editing settings through the UI can run
//! concurrently; if both went through one whole-row `update`, whichever
//! wrote last would silently clobber the other's change (the ticker
//! overwriting an admin's just-saved `active_project_limit` with a stale
//! copy it loaded a minute ago, or vice versa). A dedicated single-column
//! write for the one field the ticker ever changes avoids that read-modify-
//! write race entirely — the same "targeted operations, not whole-aggregate
//! save" principle `docs/DOMAIN.md` §7 states for every other entity in
//! this system.

use anamnesis_core::{Recurrence, SuggestionSettings, Timestamp};

/// The singleton runtime settings row. `docs/DOMAIN.md` §3's minimum set:
/// the active-project-limit invariant, the suggestion engine's tunables
/// (`docs/DOMAIN.md` §9: cooldown length and sampling weights are both
/// explicitly open, tune-later questions), and the scheduled-sweep
/// recurrence plus when it last actually ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// `count(Project.status == Active) <= active_project_limit`
    /// (`docs/DOMAIN.md` §3's global invariant).
    pub active_project_limit: u32,
    /// The suggestion engine's tunables — reused verbatim from
    /// `anamnesis_core::suggest` rather than duplicated field-for-field,
    /// since this *is* the value `anamnesis_core::suggest` takes.
    pub suggestion: SuggestionSettings,
    /// How the scheduled archive sweep repeats (`docs/DOMAIN.md` §6).
    /// `Recurrence::Never` means no scheduled sweep at all — the manual
    /// "Archive all" button still works regardless of this value.
    pub sweep_recurrence: Recurrence,
    /// When the scheduled sweep last actually ran, `None` if it never has.
    /// The sweep ticker's catch-up logic computes due-ness from this, not
    /// from process uptime, so a sweep missed while the server was down
    /// still fires on the next boot instead of being silently skipped.
    pub last_swept_at: Option<Timestamp>,
}

/// `docs/DOMAIN.md` §9 states cooldown length and sampling weights as open,
/// tune-later questions; three days is a reasonable starting point.
pub const DEFAULT_SUGGESTION_COOLDOWN_SECONDS: i64 = 3 * 24 * 60 * 60;

/// `bounce_count` at or above this earns the softer "this keeps coming
/// back" prompt (`docs/DOMAIN.md` §5).
pub const DEFAULT_HIGH_BOUNCE_THRESHOLD: u32 = 3;

/// How many projects may hold `status == Active` at once
/// (`docs/DOMAIN.md` §3's global invariant).
pub const DEFAULT_ACTIVE_PROJECT_LIMIT: u32 = 5;

impl Default for Settings {
    /// A fresh install's starting point, seeded once at bootstrap
    /// (`anamnesis-web::bootstrap`) — no scheduled sweep configured until an
    /// admin sets one, since guessing an owner's "archive day" would be
    /// worse than leaving the manual button as the only path until they
    /// choose.
    fn default() -> Self {
        Settings {
            active_project_limit: DEFAULT_ACTIVE_PROJECT_LIMIT,
            suggestion: SuggestionSettings {
                cooldown_seconds: DEFAULT_SUGGESTION_COOLDOWN_SECONDS,
                high_bounce_threshold: DEFAULT_HIGH_BOUNCE_THRESHOLD,
            },
            sweep_recurrence: Recurrence::Never,
            last_swept_at: None,
        }
    }
}
