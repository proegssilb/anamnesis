//! Requesting a suggestion (`docs/DOMAIN.md` §5) — the use case that owns
//! every side effect the pure `anamnesis_core::suggest` engine cannot
//! perform itself: assembling its inputs from the read model, deriving a
//! *stable* seed, and stamping `last_offered_at` on whatever it offers.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use anamnesis_core::policy::Role;
use anamnesis_core::{
    self as core, ColumnId, OfferItem, Outcome, Placement, SuggestionSettings, TaskSummary,
    UserId,
};

use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{BoardQuery, Clock, TaskRepository};

/// Derives the seed `anamnesis_core::suggest` samples with from
/// `(user, local_date, board-state fingerprint)` — **not** fresh entropy —
/// so the same three suggestions survive a page refresh and only change
/// once the user or the board genuinely has (`docs/DOMAIN.md` §5:
/// "Re-rolling on every F5 would let the user slot-machine for an easy
/// task, which defeats the gentle-nudge intent").
///
/// `local_date` is `(year, day-of-year)` in the user's own timezone,
/// supplied by the caller: this crate has no timezone-aware clock of its
/// own (`crate::ports::Clock` only ever hands back a bare `Timestamp`) —
/// combining it with a `crate::ports::TimezoneResolver`'s `Timezone` to get
/// a local calendar date is the caller's job, one layer up from here.
///
/// The "board-state fingerprint" is folded from every candidate's
/// scheduling-relevant fields, in an id-sorted (not insertion) order, so the
/// result does not depend on what order the repository happened to return
/// candidates in — only on what the candidates actually *are*.
pub fn derive_seed(user: &UserId, local_date: (i32, u16), candidates: &[TaskSummary]) -> u64 {
    let mut hasher = DefaultHasher::new();
    user.as_str().hash(&mut hasher);
    local_date.hash(&mut hasher);

    let mut sorted: Vec<&TaskSummary> = candidates.iter().collect();
    sorted.sort_by_key(|c| c.task_id);
    for c in sorted {
        c.task_id.as_uuid().hash(&mut hasher);
        c.archived.hash(&mut hasher);
        match c.placement {
            Placement::Below => 0u8.hash(&mut hasher),
            Placement::OnBoard { column, position } => {
                1u8.hash(&mut hasher);
                column.as_uuid().hash(&mut hasher);
                position.hash(&mut hasher);
            }
        }
        (c.project_status as u8).hash(&mut hasher);
        c.last_touched_at.unix_seconds().hash(&mut hasher);
        c.last_offered_at.map(|t| t.unix_seconds()).hash(&mut hasher);
        c.bounce_count.hash(&mut hasher);
    }
    hasher.finish()
}

/// Requests a suggestion for `user`, sized to `entry_column`'s free
/// capacity. On an [`Outcome::Offer`], stamps `last_offered_at` on every
/// task offered (`docs/DOMAIN.md` §5: "Every offer stamps
/// `last_offered_at`") — the reason this is a use case and not a direct call
/// to `anamnesis_core::suggest`.
///
/// The WIP limit is enforced simply by passing `entry_column`'s *real*
/// current occupancy (from [`BoardQuery::board_state`]) into `suggest` — the
/// same use-case-layer responsibility `crate::use_cases::task::raise_task`
/// discharges for a direct drag-and-drop move (`docs/DOMAIN.md` §7:
/// `anamnesis_core` only ever checks a count it is handed).
pub async fn request_suggestion(
    board: &dyn BoardQuery,
    task_repo: &dyn TaskRepository,
    clock: &dyn Clock,
    role: Option<Role>,
    user: &UserId,
    local_date: (i32, u16),
    entry_column: ColumnId,
    settings: &SuggestionSettings,
) -> Result<Outcome, AppError> {
    if !is_allowed(role, Action::RequestSuggestion) {
        return Err(AppError::Forbidden);
    }

    let candidates = board.suggestion_candidates().await?;
    let graph = board.blocking_graph().await?;
    let board_state = board.board_state(entry_column).await?;
    let seed = derive_seed(user, local_date, &candidates);
    let now = clock.now();

    let outcome = core::suggest(now, seed, &board_state, &candidates, &graph, settings);

    if let Outcome::Offer(offer) = &outcome {
        for item in &offer.items {
            if let OfferItem::Task(task_offer) = item {
                let Some(aggregate) = task_repo.load(task_offer.task_id).await? else {
                    continue; // vanished since the query above; nothing to stamp.
                };
                let marked = core::mark_offered(&aggregate.task, now);
                task_repo
                    .update(&marked, aggregate.task.last_touched_at)
                    .await?;
            }
        }
    }

    Ok(outcome)
}
