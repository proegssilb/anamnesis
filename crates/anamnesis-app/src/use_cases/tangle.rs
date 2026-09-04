//! Running tangle detection and reconciling it against stored state
//! (`docs/DOMAIN.md` §3). System-derived, not gated by a per-user role: a
//! `Tangle` is never edited by a person, only ever produced by this pure
//! pipeline (`anamnesis_core::detect_tangles` + `anamnesis_core::reconcile`)
//! running over the whole system's blocking graph, so there is no "which
//! project is this action scoped to" for a role check to apply against.
//!
//! **Driven by graph changes, not by reads.** Both [`run_tangle_detection`]
//! and [`resolve_frozen_tangles`] are system-wide reconciliation *writes*, so
//! neither belongs on a read path. `anamnesis_web`'s `tangles` module runs
//! them under a job lease from the two events that can change their answer —
//! creating and deleting a `blocks` edge — with a slow backstop timer behind
//! that for the cases where the event path failed. The lease is what keeps
//! them single-writer once more than one instance is running, and what lets a
//! second concurrent mutation wait for the first rather than lose its update.

use anamnesis_core::policy::Role;
use anamnesis_core::{self as core, ColumnId, Reconciliation, Tangle, TangleId};

use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{BoardQuery, Clock, IdGen, RelationshipRepository, TangleRepository};

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

/// Places a tangle on the board at `column`, freezing its membership
/// (`anamnesis_core::place_tangle`) — "untangling is work, so a tangle can
/// be placed on the board... occupying a column slot and counting against
/// that column's WIP limit exactly like a task."
///
/// The WIP-limit check mirrors `crate::use_cases::task::raise_task`:
/// `column`'s *real* current occupancy (tasks and tangles both, via
/// `BoardQuery::board_state`) is what `anamnesis_core` is handed to check
/// against — core itself only ever checks a count it is given.
pub async fn place_tangle(
    tangle_repo: &dyn TangleRepository,
    board: &dyn BoardQuery,
    role: Option<Role>,
    tangle_id: TangleId,
    column: ColumnId,
) -> Result<Tangle, AppError> {
    if !is_allowed(role, Action::PlaceTangle) {
        return Err(AppError::Forbidden);
    }
    let tangle = tangle_repo
        .load(tangle_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let state = board.board_state(column).await?;
    if let Some(limit) = state.wip_limit
        && state.current_count >= limit
    {
        return Err(AppError::WipLimitExceeded);
    }
    let placed = core::place_tangle(&tangle, column, state.current_count)?;
    tangle_repo.update(&placed).await?;
    Ok(placed)
}

/// Drops a tangle back below the horizon, unfreezing it
/// (`anamnesis_core::drop_tangle`) — detection is free to refresh or
/// dissolve it again, same as any other below-the-horizon tangle.
pub async fn drop_tangle(
    tangle_repo: &dyn TangleRepository,
    role: Option<Role>,
    tangle_id: TangleId,
) -> Result<Tangle, AppError> {
    if !is_allowed(role, Action::PlaceTangle) {
        return Err(AppError::Forbidden);
    }
    let tangle = tangle_repo
        .load(tangle_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let dropped = core::drop_tangle(&tangle)?;
    tangle_repo.update(&dropped).await?;
    Ok(dropped)
}

/// Resolves every frozen, on-board tangle whose frozen task set no longer
/// contains a cycle in the live blocking graph (`anamnesis_core::
/// resolve_frozen_tangle`) — checked directly against the graph, never
/// against a fresh [`run_tangle_detection`] pass, which never touches a
/// frozen tangle at all.
///
/// `done_column` names the board's `is_done` column, if one is configured
/// (`docs/DOMAIN.md`: a resolving tangle "moves to the `is_done` column so
/// the user sees the knot closed"); `None` still resolves every qualifying
/// tangle, just without moving it anywhere.
pub async fn resolve_frozen_tangles(
    relationship_repo: &dyn RelationshipRepository,
    tangle_repo: &dyn TangleRepository,
    board: &dyn BoardQuery,
    clock: &dyn Clock,
    done_column: Option<ColumnId>,
) -> Result<Vec<Tangle>, AppError> {
    let relationships = relationship_repo.list_blocking().await?;
    let kinds = [core::builtin_blocks()];
    let now = clock.now();
    let active = tangle_repo.list_active().await?;

    let mut resolved = Vec::new();
    for tangle in active.iter().filter(|t| t.frozen) {
        let done = match done_column {
            Some(column) => Some((column, board.count_on_column(column).await?)),
            None => None,
        };
        if let Some(closed) = core::resolve_frozen_tangle(tangle, &relationships, &kinds, now, done)
        {
            tangle_repo.update(&closed).await?;
            resolved.push(closed);
        }
    }
    Ok(resolved)
}
