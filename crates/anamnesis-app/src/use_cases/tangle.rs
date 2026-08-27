//! Running tangle detection and reconciling it against stored state
//! (`docs/DOMAIN.md` §3). System-derived, not gated by a per-user role: a
//! `Tangle` is never edited by a person, only ever produced by this pure
//! pipeline (`anamnesis_core::detect_tangles` + `anamnesis_core::reconcile`)
//! running over the whole system's blocking graph, so there is no "which
//! project is this action scoped to" for a role check to apply against.
//! Real deployments would run this from a scheduled job, the same way a
//! sweep ticker runs `crate::use_cases::archive_done_tasks`.

use anamnesis_core::{self as core, Reconciliation, TangleId};

use crate::error::AppError;
use crate::ports::{Clock, IdGen, RelationshipRepository, TangleRepository};

/// Detects every tangle in the current blocking graph and reconciles it
/// against what is already stored: newly detected tangles are inserted,
/// resolved ones are stamped and persisted, and tangles still holding are
/// left untouched (`anamnesis_core::reconcile`'s entire mutation surface).
pub async fn run_tangle_detection(
    relationship_repo: &dyn RelationshipRepository,
    tangle_repo: &dyn TangleRepository,
    ids: &dyn IdGen,
    clock: &dyn Clock,
) -> Result<Reconciliation, AppError> {
    let relationships = relationship_repo.list_blocking().await?;
    // `list_blocking` already returns only built-in-`blocks` edges, so a
    // single-element kinds list naming that one built-in kind is enough for
    // `detect_tangles`'s own (redundant, but harmless) filter to pass
    // everything through.
    let detected = core::detect_tangles(&relationships, &[core::builtin_blocks()]);
    let previous = tangle_repo.list_active().await?;
    let now = clock.now();
    let fresh_ids = std::iter::repeat_with(|| TangleId::new(ids.next()));

    let reconciliation = core::reconcile(&detected, &previous, now, fresh_ids);

    for tangle in &reconciliation.newly_detected {
        tangle_repo.insert(tangle).await?;
    }
    for tangle in &reconciliation.resolved {
        tangle_repo.update(tangle).await?;
    }

    Ok(reconciliation)
}
