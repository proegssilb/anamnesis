//! The suggestion engine — "the soul of the product" (`docs/DOMAIN.md` §5).
//!
//! A pure, total function of `(now, seed, board, candidates, graph,
//! settings)`. No clock read, no RNG call: `now` is a parameter and `seed` is
//! entropy handed in from outside, run through a small deterministic PRNG
//! ([`SplitMix64`]) so the same seed always produces the same offer — the
//! thing that keeps three suggestions stable across a page refresh instead
//! of re-rolling into a slot machine.
//!
//! Three outcomes, one hard product rule:
//!
//! - [`Outcome::Full`] — the board is at its WIP limit. **Say nothing at
//!   all.** A full board means the user is already carrying what they agreed
//!   to carry; the system does not sass them for it.
//! - [`Outcome::Offer`] — there is room and something eligible: up to three
//!   items, sampled (never top-N-ranked) so every eligible task keeps a
//!   non-zero chance of coming up, however long it has sat untouched.
//! - [`Outcome::Stuck`] — there is room but *nothing* eligible. This is the
//!   `Err` arm: silence here would look like a broken app, because the user
//!   can see the empty slot. [`Blockage`] names the concrete reason.

use std::collections::BTreeSet;

use crate::ids::{TaskId, Timestamp};
use crate::placement::Placement;
use crate::project::ProjectStatus;
use crate::tangle::Tangle;
use crate::task::Task;

/// A small, fixed-output, seeded PRNG — SplitMix64. Not cryptographic, not
/// even statistically special; it exists purely so this crate can sample
/// deterministically from an externally supplied `seed` without depending on
/// (or wrapping) `rand` or reading any entropy source. Same seed, same
/// stream, forever.
struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform `f64` in `[0, 1)`.
    fn next_unit(&mut self) -> f64 {
        // Top 53 bits -> the full mantissa of an f64, so every representable
        // value in [0, 1) is reachable with even density.
        let top53 = self.next_u64() >> 11;
        (top53 as f64) * (1.0 / (1u64 << 53) as f64)
    }
}

/// Everything the engine needs to know about one candidate task — a summary,
/// not the full `Task`: exactly the scheduling-relevant fields, since that is
/// all a suggestion decision ever looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSummary {
    pub task_id: TaskId,
    pub archived: bool,
    pub placement: Placement,
    pub project_status: ProjectStatus,
    pub last_touched_at: Timestamp,
    pub last_offered_at: Option<Timestamp>,
    pub bounce_count: u32,
}

impl TaskSummary {
    /// Builds a summary directly from a [`Task`] plus the status of its
    /// owning project — the two pieces of information a `Task` cannot supply
    /// about itself (core loads no project here; the caller already has
    /// both loaded).
    pub fn from_task(task: &Task, project_status: ProjectStatus) -> Self {
        TaskSummary {
            task_id: task.id,
            archived: task.archived_at.is_some(),
            placement: task.placement,
            project_status,
            last_touched_at: task.last_touched_at,
            last_offered_at: task.last_offered_at,
            bounce_count: task.bounce_count,
        }
    }
}

/// The engine's view of the blocking graph and the tangles detected over it:
/// enough to decide "unblocked" and "not in an unresolved tangle" for every
/// candidate, and to offer a tangle in place of its knotted tasks.
///
/// `tangled_task_ids` and `tangles` are deliberately independent inputs
/// rather than one derived from the other. Ordinarily every id in
/// `tangled_task_ids` traces back to some `Tangle` in `tangles` — but not
/// always: a tangle that has already been *accepted* onto the board occupies
/// a slot as its own work item and must not be offered a second time, yet
/// its member tasks stay excluded from individual suggestion until the knot
/// actually resolves. The caller (which owns board state this crate does
/// not) curates `tangles` down to the ones still worth offering; this module
/// only trusts that whatever is in `tangled_task_ids` should stay excluded.
#[derive(Debug, Clone, Default)]
pub struct BlockingGraph {
    /// `(blocker, blocked)` pairs — a `blocks` edge from `blocker` to
    /// `blocked`, exactly as `crate::relationship::builtin_blocks` labels it.
    pub edges: Vec<(TaskId, TaskId)>,
    /// Task ids currently sitting in an `is_done` column. A blocker not in
    /// this set still blocks; a blocker in it no longer does.
    pub done_task_ids: BTreeSet<TaskId>,
    /// Every task id currently bound up in an unresolved tangle, regardless
    /// of whether that tangle is currently offerable (see the struct doc).
    pub tangled_task_ids: BTreeSet<TaskId>,
    /// Unresolved tangles currently available to offer in place of their
    /// (excluded) member tasks. A tangle with `resolved_at: Some(_)` here is
    /// ignored defensively, as is one already on the board per the struct
    /// doc — both are the caller's responsibility to have excluded already.
    pub tangles: Vec<Tangle>,
}

impl BlockingGraph {
    /// True if `task_id` has an incoming `blocks` edge from a task that is
    /// not (yet) done.
    fn is_blocked(&self, task_id: TaskId) -> bool {
        self.edges
            .iter()
            .any(|&(blocker, blocked)| blocked == task_id && !self.done_task_ids.contains(&blocker))
    }

    /// The unresolved subset of `tangles` — the ones actually offerable.
    fn active_tangles(&self) -> impl Iterator<Item = &Tangle> {
        self.tangles.iter().filter(|t| t.is_active())
    }
}

/// The engine's tunable knobs (`docs/DOMAIN.md` §9: cooldown length and
/// sampling weights are both explicitly open, tune-later questions — these
/// are the parameters that tuning would adjust).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuggestionSettings {
    /// How long, in seconds, a task stays off future offers after being
    /// offered — "declining something does not immediately re-offer it."
    pub cooldown_seconds: i64,
    /// `bounce_count` at or above this value gets the softer "this keeps
    /// coming back" prompt (via [`TaskOffer::high_bounce`]) instead of the
    /// plain one.
    pub high_bounce_threshold: u32,
}

/// The current state of the board relevant to sizing an offer: how big the
/// entry column's WIP limit is, and how many tasks are on it right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoardState {
    pub wip_limit: Option<u32>,
    pub current_count: u32,
}

impl BoardState {
    /// Free capacity, or `None` if the column carries no WIP limit at all.
    fn free_slots(&self) -> Option<u32> {
        self.wip_limit
            .map(|limit| limit.saturating_sub(self.current_count))
    }
}

/// Why one suggestion — a task, specifically, as opposed to a tangle — was
/// picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionReason {
    /// Sampled from the recency-weighted distribution.
    NextUp,
    /// Sampled from the staleness-weighted distribution over the older tail.
    Forgotten,
}

/// One task offered up, with the reason it was picked and whether it has
/// bounced enough to earn the softer prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskOffer {
    pub task_id: TaskId,
    pub reason: SuggestionReason,
    /// True when `bounce_count >= settings.high_bounce_threshold`: the UI
    /// should use "this keeps coming back — break it up, or let it go?"
    /// rather than the plain prompt.
    pub high_bounce: bool,
}

/// One item in an [`Offer`]: either a plain task, or a tangle offered in
/// place of the (excluded) tasks that make it up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfferItem {
    Task(TaskOffer),
    Tangle(Tangle),
}

/// Up to three suggestions, sized to the board's free capacity.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Offer {
    pub items: Vec<OfferItem>,
}

/// Why [`suggest`] found room on the board but nothing to put in it — the
/// `Err` arm. Checked as an ordered funnel from the most fundamental cause to
/// the most specific (see the module's internal `diagnose`): if the backlog
/// is empty there is no point checking whether anything is blocked, and so
/// on down to cooldown, which only applies to what would otherwise have
/// qualified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blockage {
    /// There is no task below the horizon at all.
    BacklogEmpty,
    /// The backlog is non-empty, but none of it belongs to an `Active`
    /// project.
    NoActiveProject,
    /// Every otherwise-eligible task has an incoming `blocks` edge from a
    /// task that is not done.
    AllBlocked,
    /// Every otherwise-eligible task is bound up in an unresolved tangle, and
    /// (unusually) no tangle is available to offer in its place.
    AllTangled,
    /// Every otherwise-eligible task was offered too recently.
    AllOnCooldown,
}

/// The result of asking the engine for a suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The board is at its WIP limit. Show nothing.
    Full,
    /// Room, and something eligible: up to three items.
    Offer(Offer),
    /// Room, but nothing eligible — see [`Blockage`] for why.
    Stuck(Blockage),
}

fn is_eligible(
    candidate: &TaskSummary,
    now: Timestamp,
    graph: &BlockingGraph,
    tangled_ids: &BTreeSet<TaskId>,
    settings: &SuggestionSettings,
) -> bool {
    !candidate.archived
        && candidate.placement.is_below()
        && candidate.project_status == ProjectStatus::Active
        && !graph.is_blocked(candidate.task_id)
        && !tangled_ids.contains(&candidate.task_id)
        && off_cooldown(candidate, now, settings)
}

fn off_cooldown(candidate: &TaskSummary, now: Timestamp, settings: &SuggestionSettings) -> bool {
    match candidate.last_offered_at {
        None => true,
        Some(offered_at) => {
            now.unix_seconds() - offered_at.unix_seconds() >= settings.cooldown_seconds
        }
    }
}

/// Diagnoses why nothing is eligible, as an ordered funnel — only called once
/// the caller already knows both "eligible tasks" and "offerable tangles" are
/// empty, so every branch here is about *why* rather than *whether*.
fn diagnose(
    candidates: &[TaskSummary],
    graph: &BlockingGraph,
    tangled_ids: &BTreeSet<TaskId>,
) -> Blockage {
    let live: Vec<&TaskSummary> = candidates.iter().filter(|c| !c.archived).collect();
    let below: Vec<&&TaskSummary> = live.iter().filter(|c| c.placement.is_below()).collect();
    if below.is_empty() {
        return Blockage::BacklogEmpty;
    }
    let active: Vec<&&&TaskSummary> = below
        .iter()
        .filter(|c| c.project_status == ProjectStatus::Active)
        .collect();
    if active.is_empty() {
        return Blockage::NoActiveProject;
    }
    let unblocked: Vec<_> = active
        .iter()
        .filter(|c| !graph.is_blocked(c.task_id))
        .collect();
    if unblocked.is_empty() {
        return Blockage::AllBlocked;
    }
    let untangled: Vec<_> = unblocked
        .iter()
        .filter(|c| !tangled_ids.contains(&c.task_id))
        .collect();
    if untangled.is_empty() {
        return Blockage::AllTangled;
    }
    // `untangled` is non-empty, yet the caller found no eligible task: the
    // only remaining rule left to fail is cooldown.
    Blockage::AllOnCooldown
}

/// One item competing for a slot in the offer: either a real candidate task,
/// or a tangle standing in for its (individually ineligible) members.
enum PoolItem<'a> {
    Task(&'a TaskSummary),
    Tangle(&'a Tangle),
}

impl PoolItem<'_> {
    /// The timestamp this item is weighted relative to: a task's own
    /// `last_touched_at`, or — since a `Tangle` carries no such field — the
    /// moment it was detected, which plays the same "how long has this been
    /// sitting" role for weighting purposes.
    fn reference_time(&self) -> Timestamp {
        match self {
            PoolItem::Task(t) => t.last_touched_at,
            PoolItem::Tangle(t) => t.detected_at,
        }
    }

    fn task_id(&self) -> Option<TaskId> {
        match self {
            PoolItem::Task(t) => Some(t.task_id),
            PoolItem::Tangle(_) => None,
        }
    }

    fn tangle_id(&self) -> Option<crate::ids::TangleId> {
        match self {
            PoolItem::Task(_) => None,
            PoolItem::Tangle(t) => Some(t.id),
        }
    }
}

fn age_seconds(now: Timestamp, reference: Timestamp) -> i64 {
    (now.unix_seconds() - reference.unix_seconds()).max(0)
}

/// Recency weight: larger for more recently touched items. Strictly
/// positive for every item, however old — the anti-starvation guarantee
/// that every eligible item has *some* chance of being drawn as "next up".
fn recency_weight(now: Timestamp, reference: Timestamp) -> f64 {
    1.0 / (1.0 + age_seconds(now, reference) as f64)
}

/// Staleness weight: larger for older items. Strictly positive for every
/// item — the same guarantee, for the "forgotten" slot.
fn staleness_weight(now: Timestamp, reference: Timestamp) -> f64 {
    1.0 + age_seconds(now, reference) as f64
}

/// Draws one index out of `weights` via roulette-wheel selection. Every
/// weight must be `> 0.0` (both weight functions above guarantee this for
/// any real timestamp); falls back to a uniform draw only in the
/// (unreachable in practice) case where the total is not positive.
fn weighted_index(rng: &mut SplitMix64, weights: &[f64]) -> usize {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return (rng.next_u64() as usize) % weights.len();
    }
    let mut r = rng.next_unit() * total;
    for (i, w) in weights.iter().enumerate() {
        if r < *w {
            return i;
        }
        r -= w;
    }
    weights.len() - 1
}

/// How many "next up" / "forgotten" slots make up an offer of this size.
/// A full offer (3 slots) is 2 next-up + 1 forgotten, per `docs/DOMAIN.md`
/// §5. For a smaller offer, this crate resolves that (undocumented) edge by
/// shrinking the forgotten slot first: 2 -> (1, 1), 1 -> (1, 0), 0 -> (0, 0).
/// Rationale: with only one slot free, the single most defensible thing to
/// surface is something plausibly still fresh in mind, not a deep-backlog
/// item that would need more context to act on.
fn composition(offer_size: u32) -> (u32, u32) {
    match offer_size {
        0 => (0, 0),
        1 => (1, 0),
        2 => (1, 1),
        _ => (2, 1),
    }
}

/// The suggestion engine (`docs/DOMAIN.md` §5).
pub fn suggest(
    now: Timestamp,
    seed: u64,
    board: &BoardState,
    candidates: &[TaskSummary],
    graph: &BlockingGraph,
    settings: &SuggestionSettings,
) -> Outcome {
    let free = match board.free_slots() {
        Some(0) => return Outcome::Full,
        Some(free) => free,
        None => 3, // unlimited WIP: never Full, offer size still caps at 3.
    };
    let offer_size = free.min(3);

    let tangled_ids = &graph.tangled_task_ids;
    let eligible_tasks: Vec<&TaskSummary> = candidates
        .iter()
        .filter(|c| is_eligible(c, now, graph, tangled_ids, settings))
        .collect();
    let offerable_tangles: Vec<&Tangle> = graph.active_tangles().collect();

    if eligible_tasks.is_empty() && offerable_tangles.is_empty() {
        return Outcome::Stuck(diagnose(candidates, graph, tangled_ids));
    }

    let mut pool: Vec<PoolItem> = eligible_tasks
        .into_iter()
        .map(PoolItem::Task)
        .chain(offerable_tangles.into_iter().map(PoolItem::Tangle))
        .collect();

    // The "older tail" the forgotten slot samples from: the staler half of
    // the pool by reference time (ties broken by a stable secondary key so
    // the split is deterministic). A pool of 0 or 1 items is its own tail.
    let mut by_age: Vec<usize> = (0..pool.len()).collect();
    by_age.sort_by(|&a, &b| {
        age_seconds(now, pool[a].reference_time())
            .cmp(&age_seconds(now, pool[b].reference_time()))
            .reverse()
            .then_with(|| pool_sort_key(&pool[a]).cmp(&pool_sort_key(&pool[b])))
    });
    let tail_len = by_age.len().div_ceil(2).max(1.min(by_age.len()));
    let tail: BTreeSet<usize> = by_age.into_iter().take(tail_len).collect();

    let (next_up_count, forgotten_count) = composition(offer_size);
    let mut rng = SplitMix64::new(seed);
    let mut drawn: Vec<(usize, SuggestionReason)> = Vec::new();
    let mut available: Vec<usize> = (0..pool.len()).collect();

    for _ in 0..forgotten_count {
        let tail_available: Vec<usize> = available
            .iter()
            .copied()
            .filter(|i| tail.contains(i))
            .collect();
        if tail_available.is_empty() {
            break;
        }
        let weights: Vec<f64> = tail_available
            .iter()
            .map(|&i| staleness_weight(now, pool[i].reference_time()))
            .collect();
        let pick = tail_available[weighted_index(&mut rng, &weights)];
        drawn.push((pick, SuggestionReason::Forgotten));
        available.retain(|&i| i != pick);
    }

    for _ in 0..next_up_count {
        if available.is_empty() {
            break;
        }
        let weights: Vec<f64> = available
            .iter()
            .map(|&i| recency_weight(now, pool[i].reference_time()))
            .collect();
        let pick = available[weighted_index(&mut rng, &weights)];
        drawn.push((pick, SuggestionReason::NextUp));
        available.retain(|&i| i != pick);
    }

    // Stable output order: reason the item was drawn for doesn't need to be
    // exposed as ordering, but a deterministic order (by original pool
    // position) keeps `Offer` equality meaningful for the seed-stability
    // test regardless of internal draw order.
    drawn.sort_by_key(|&(i, _)| i);

    let items = drawn
        .into_iter()
        .map(|(i, reason)| match &pool[i] {
            PoolItem::Task(_) => {
                let summary = eligible_task_by_index(&mut pool, i);
                OfferItem::Task(TaskOffer {
                    task_id: summary.task_id,
                    reason,
                    high_bounce: summary.bounce_count >= settings.high_bounce_threshold,
                })
            }
            PoolItem::Tangle(t) => OfferItem::Tangle((*t).clone()),
        })
        .collect();

    Outcome::Offer(Offer { items })
}

/// A stable secondary sort key for age ties in the tail split: a task's own
/// id, or a tangle's id — both `Ord`, giving a total order without favouring
/// one kind over the other by any meaning beyond "deterministic".
fn pool_sort_key(item: &PoolItem) -> u128 {
    item.task_id()
        .map(|id| id.as_uuid().as_u128())
        .or_else(|| item.tangle_id().map(|id| id.as_uuid().as_u128()))
        .unwrap_or(0)
}

fn eligible_task_by_index<'a>(pool: &mut [PoolItem<'a>], i: usize) -> &'a TaskSummary {
    match pool[i] {
        PoolItem::Task(t) => t,
        PoolItem::Tangle(_) => unreachable!("caller only calls this for a Task pool item"),
    }
}

/// Stamps `last_offered_at`. Called for every task placed into an
/// [`Offer`] — `docs/DOMAIN.md` §5: "Every offer stamps `last_offered_at`."
pub fn mark_offered(task: &Task, now: Timestamp) -> Task {
    Task {
        last_offered_at: Some(now),
        ..task.clone()
    }
}

/// Moves a task from the board back below the horizon, accounting for a
/// bounce if the column it is leaving is not `is_done` —
/// `docs/DOMAIN.md` §5: "moving `OnBoard -> Below` without reaching an
/// `is_done` column increments `bounce_count` and stamps `last_bounced_at`."
///
/// `left_a_done_column` is supplied by the caller (this module loads no
/// `Column`), and should be `false` for a task pulled back down mid-flight
/// and `true` for one leaving a `Done`-like column — though in the latter
/// case the caller would ordinarily archive or otherwise finish the task
/// rather than sink it, so this is really the "gave up on it" path.
pub fn bounce_to_below(
    task: &Task,
    left_a_done_column: bool,
    now: Timestamp,
) -> Result<Task, crate::error::DomainError> {
    let moved = crate::task::move_placement(task, Placement::Below, now)?;
    if left_a_done_column {
        Ok(moved)
    } else {
        Ok(Task {
            bounce_count: moved.bounce_count + 1,
            last_bounced_at: Some(now),
            ..moved
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ColumnId, ProjectId, TangleId};
    use std::collections::HashSet as StdHashSet;
    use uuid::Uuid;

    fn tid(n: u128) -> TaskId {
        TaskId::new(Uuid::from_u128(n))
    }

    fn ts(secs: i64) -> Timestamp {
        Timestamp::from_unix_seconds(secs).unwrap()
    }

    fn col(n: u128) -> ColumnId {
        ColumnId::new(Uuid::from_u128(n))
    }

    fn settings() -> SuggestionSettings {
        SuggestionSettings {
            cooldown_seconds: 3 * 24 * 3600,
            high_bounce_threshold: 3,
        }
    }

    fn below_task(id: u128, last_touched: i64) -> TaskSummary {
        TaskSummary {
            task_id: tid(id),
            archived: false,
            placement: Placement::Below,
            project_status: ProjectStatus::Active,
            last_touched_at: ts(last_touched),
            last_offered_at: None,
            bounce_count: 0,
        }
    }

    fn empty_graph() -> BlockingGraph {
        BlockingGraph::default()
    }

    // --- Full: silence at the WIP limit ---

    #[test]
    fn full_board_returns_full_and_nothing_else() {
        let board = BoardState {
            wip_limit: Some(3),
            current_count: 3,
        };
        let candidates = vec![below_task(1, 0)];
        let outcome = suggest(
            ts(100),
            42,
            &board,
            &candidates,
            &empty_graph(),
            &settings(),
        );
        assert_eq!(outcome, Outcome::Full);
    }

    #[test]
    fn a_board_over_its_limit_is_also_full() {
        let board = BoardState {
            wip_limit: Some(3),
            current_count: 5,
        };
        let candidates = vec![below_task(1, 0)];
        let outcome = suggest(
            ts(100),
            42,
            &board,
            &candidates,
            &empty_graph(),
            &settings(),
        );
        assert_eq!(outcome, Outcome::Full);
    }

    #[test]
    fn full_takes_priority_even_when_nothing_would_have_been_eligible_anyway() {
        // The board is full AND the backlog is empty: still Full, not Stuck.
        // Full is checked first and unconditionally.
        let board = BoardState {
            wip_limit: Some(1),
            current_count: 1,
        };
        let outcome = suggest(ts(100), 42, &board, &[], &empty_graph(), &settings());
        assert_eq!(outcome, Outcome::Full);
    }

    // --- Stuck: each Blockage variant reachable for the right reason ---

    fn room(current_count: u32) -> BoardState {
        BoardState {
            wip_limit: Some(3),
            current_count,
        }
    }

    #[test]
    fn stuck_backlog_empty_when_there_is_no_task_below_the_horizon_at_all() {
        let candidates = vec![TaskSummary {
            task_id: tid(1),
            archived: false,
            placement: Placement::OnBoard {
                column: col(1),
                position: 0,
            },
            project_status: ProjectStatus::Active,
            last_touched_at: ts(0),
            last_offered_at: None,
            bounce_count: 0,
        }];
        let outcome = suggest(
            ts(100),
            1,
            &room(0),
            &candidates,
            &empty_graph(),
            &settings(),
        );
        assert_eq!(outcome, Outcome::Stuck(Blockage::BacklogEmpty));
    }

    #[test]
    fn stuck_backlog_empty_with_no_candidates_at_all() {
        let outcome = suggest(ts(100), 1, &room(0), &[], &empty_graph(), &settings());
        assert_eq!(outcome, Outcome::Stuck(Blockage::BacklogEmpty));
    }

    #[test]
    fn stuck_no_active_project_when_the_backlog_belongs_to_pending_or_complete_projects() {
        let candidates = vec![
            TaskSummary {
                project_status: ProjectStatus::Pending,
                ..below_task(1, 0)
            },
            TaskSummary {
                project_status: ProjectStatus::Complete,
                ..below_task(2, 0)
            },
        ];
        let outcome = suggest(
            ts(100),
            1,
            &room(0),
            &candidates,
            &empty_graph(),
            &settings(),
        );
        assert_eq!(outcome, Outcome::Stuck(Blockage::NoActiveProject));
    }

    #[test]
    fn stuck_all_blocked_when_every_active_backlog_task_has_an_unfinished_blocker() {
        let candidates = vec![below_task(1, 0), below_task(2, 0)];
        let graph = BlockingGraph {
            edges: vec![(tid(99), tid(1)), (tid(98), tid(2))],
            done_task_ids: BTreeSet::new(),
            tangled_task_ids: BTreeSet::new(),
            tangles: vec![],
        };
        let outcome = suggest(ts(100), 1, &room(0), &candidates, &graph, &settings());
        assert_eq!(outcome, Outcome::Stuck(Blockage::AllBlocked));
    }

    #[test]
    fn a_blocker_that_is_done_no_longer_blocks() {
        let candidates = vec![below_task(1, 0)];
        let graph = BlockingGraph {
            edges: vec![(tid(99), tid(1))],
            done_task_ids: [tid(99)].into_iter().collect(),
            tangled_task_ids: BTreeSet::new(),
            tangles: vec![],
        };
        let outcome = suggest(ts(100), 1, &room(0), &candidates, &graph, &settings());
        assert!(matches!(outcome, Outcome::Offer(_)));
    }

    fn tangle(id: u128, task_ids: &[u128], detected_at: i64) -> Tangle {
        Tangle {
            id: TangleId::new(Uuid::from_u128(id)),
            task_ids: task_ids.iter().map(|&n| tid(n)).collect(),
            fingerprint: crate::tangle::Fingerprint::of(
                &task_ids.iter().map(|&n| tid(n)).collect(),
            ),
            detected_at: ts(detected_at),
            resolved_at: None,
        }
    }

    #[test]
    fn stuck_all_tangled_when_the_tangle_is_already_on_the_board_and_not_offerable() {
        // Both tasks are bound up in an unresolved tangle (`tangled_task_ids`
        // excludes them from individual eligibility), but that Tangle has
        // already been accepted onto the board as its own work item — the
        // caller therefore does not include it in `tangles` (offering it a
        // second time would be redundant). Nothing is left to suggest, and
        // the reason is specifically "everything eligible is knotted", not
        // any of the other four.
        let candidates = vec![below_task(1, 0), below_task(2, 0)];
        let graph = BlockingGraph {
            edges: vec![],
            done_task_ids: BTreeSet::new(),
            tangled_task_ids: [tid(1), tid(2)].into_iter().collect(),
            tangles: vec![],
        };
        let outcome = suggest(ts(100), 1, &room(0), &candidates, &graph, &settings());
        assert_eq!(outcome, Outcome::Stuck(Blockage::AllTangled));
    }

    #[test]
    fn stuck_all_on_cooldown_when_everything_eligible_was_offered_too_recently() {
        let candidates = vec![
            TaskSummary {
                last_offered_at: Some(ts(99)),
                ..below_task(1, 0)
            },
            TaskSummary {
                last_offered_at: Some(ts(99)),
                ..below_task(2, 0)
            },
        ];
        let outcome = suggest(
            ts(100),
            1,
            &room(0),
            &candidates,
            &empty_graph(),
            &settings(),
        );
        assert_eq!(outcome, Outcome::Stuck(Blockage::AllOnCooldown));
    }

    #[test]
    fn a_task_offered_long_enough_ago_is_off_cooldown() {
        let candidates = vec![TaskSummary {
            last_offered_at: Some(ts(0)),
            ..below_task(1, 0)
        }];
        let outcome = suggest(
            ts(1_000_000),
            1,
            &room(0),
            &candidates,
            &empty_graph(),
            &settings(),
        );
        assert!(matches!(outcome, Outcome::Offer(_)));
    }

    // --- Tangle offered in place of its knotted tasks ---

    #[test]
    fn tangled_tasks_are_excluded_and_the_tangle_is_offered_instead() {
        let candidates = vec![
            below_task(1, 0),
            below_task(2, 0),
            below_task(3, 0),
            below_task(4, 0),
        ];
        let graph = BlockingGraph {
            edges: vec![
                (tid(1), tid(2)),
                (tid(2), tid(3)),
                (tid(3), tid(4)),
                (tid(4), tid(1)),
            ],
            done_task_ids: BTreeSet::new(),
            tangled_task_ids: [tid(1), tid(2), tid(3), tid(4)].into_iter().collect(),
            tangles: vec![tangle(1, &[1, 2, 3, 4], 0)],
        };
        let outcome = suggest(ts(100), 1, &room(0), &candidates, &graph, &settings());
        match outcome {
            Outcome::Offer(offer) => {
                assert_eq!(offer.items.len(), 1);
                match &offer.items[0] {
                    OfferItem::Tangle(t) => {
                        assert_eq!(t.task_ids.len(), 4);
                    }
                    OfferItem::Task(_) => panic!("expected the tangle, not an individual task"),
                }
            }
            other => panic!("expected an Offer, got {other:?}"),
        }
    }

    #[test]
    fn an_eligible_task_outside_the_tangle_can_still_be_offered_alongside_it() {
        let candidates = vec![below_task(1, 0), below_task(2, 0), below_task(99, 5)];
        let graph = BlockingGraph {
            edges: vec![(tid(1), tid(2)), (tid(2), tid(1))],
            done_task_ids: BTreeSet::new(),
            tangled_task_ids: [tid(1), tid(2)].into_iter().collect(),
            tangles: vec![tangle(1, &[1, 2], 0)],
        };
        let outcome = suggest(ts(100), 7, &room(0), &candidates, &graph, &settings());
        let Outcome::Offer(offer) = outcome else {
            panic!("expected an Offer")
        };
        let has_tangle = offer
            .items
            .iter()
            .any(|i| matches!(i, OfferItem::Tangle(_)));
        assert!(has_tangle);
        // Neither individually-tangled task (1 or 2) is ever offered on its
        // own -- only the tangle stands in for them.
        for item in &offer.items {
            if let OfferItem::Task(t) = item {
                assert!(t.task_id != tid(1) && t.task_id != tid(2));
            }
        }
    }

    #[test]
    fn a_resolved_tangle_is_never_offered_and_its_tasks_become_eligible_again() {
        let candidates = vec![below_task(1, 0), below_task(2, 0)];
        let mut t = tangle(1, &[1, 2], 0);
        t.resolved_at = Some(ts(50));
        let graph = BlockingGraph {
            edges: vec![],
            done_task_ids: BTreeSet::new(),
            // Resolved: the caller no longer excludes 1/2 as tangled.
            tangled_task_ids: BTreeSet::new(),
            tangles: vec![t],
        };
        let outcome = suggest(ts(100), 1, &room(0), &candidates, &graph, &settings());
        let Outcome::Offer(offer) = outcome else {
            panic!("expected an Offer")
        };
        assert!(offer.items.iter().all(|i| matches!(i, OfferItem::Task(_))));
    }

    // --- Offer size shrinks with free slots ---

    #[test]
    fn offer_size_matches_free_slots_when_fewer_than_three() {
        let candidates: Vec<TaskSummary> = (1..=10).map(|n| below_task(n, n as i64)).collect();
        let board = BoardState {
            wip_limit: Some(3),
            current_count: 2, // only 1 free
        };
        let outcome = suggest(
            ts(1_000_000),
            5,
            &board,
            &candidates,
            &empty_graph(),
            &settings(),
        );
        let Outcome::Offer(offer) = outcome else {
            panic!("expected an Offer")
        };
        assert_eq!(offer.items.len(), 1);
    }

    #[test]
    fn offer_size_is_capped_at_three_even_with_many_free_slots() {
        let candidates: Vec<TaskSummary> = (1..=10).map(|n| below_task(n, n as i64)).collect();
        let board = BoardState {
            wip_limit: Some(20),
            current_count: 0,
        };
        let outcome = suggest(
            ts(1_000_000),
            5,
            &board,
            &candidates,
            &empty_graph(),
            &settings(),
        );
        let Outcome::Offer(offer) = outcome else {
            panic!("expected an Offer")
        };
        assert_eq!(offer.items.len(), 3);
    }

    #[test]
    fn offer_never_exceeds_the_number_of_eligible_items_available() {
        let candidates = vec![below_task(1, 0)];
        let board = BoardState {
            wip_limit: Some(20),
            current_count: 0,
        };
        let outcome = suggest(
            ts(1_000_000),
            5,
            &board,
            &candidates,
            &empty_graph(),
            &settings(),
        );
        let Outcome::Offer(offer) = outcome else {
            panic!("expected an Offer")
        };
        assert_eq!(offer.items.len(), 1);
    }

    // --- Bounce flagging ---

    #[test]
    fn a_high_bounce_candidate_is_flagged_in_its_offer() {
        let candidates = vec![TaskSummary {
            bounce_count: 5,
            ..below_task(1, 0)
        }];
        let board = BoardState {
            wip_limit: Some(3),
            current_count: 0,
        };
        let outcome = suggest(ts(100), 1, &board, &candidates, &empty_graph(), &settings());
        let Outcome::Offer(offer) = outcome else {
            panic!("expected an Offer")
        };
        let OfferItem::Task(t) = &offer.items[0] else {
            panic!("expected a task item")
        };
        assert!(t.high_bounce);
    }

    #[test]
    fn a_low_bounce_candidate_is_not_flagged() {
        let candidates = vec![below_task(1, 0)];
        let board = BoardState {
            wip_limit: Some(3),
            current_count: 0,
        };
        let outcome = suggest(ts(100), 1, &board, &candidates, &empty_graph(), &settings());
        let Outcome::Offer(offer) = outcome else {
            panic!("expected an Offer")
        };
        let OfferItem::Task(t) = &offer.items[0] else {
            panic!("expected a task item")
        };
        assert!(!t.high_bounce);
    }

    // --- Seed stability: the same seed returns the identical offer ---

    #[test]
    fn the_same_seed_returns_the_identical_offer_across_repeated_calls() {
        let candidates: Vec<TaskSummary> = (1..=30).map(|n| below_task(n, n as i64 * 37)).collect();
        let board = BoardState {
            wip_limit: Some(3),
            current_count: 0,
        };
        let first = suggest(
            ts(500_000),
            424242,
            &board,
            &candidates,
            &empty_graph(),
            &settings(),
        );
        for _ in 0..10 {
            let again = suggest(
                ts(500_000),
                424242,
                &board,
                &candidates,
                &empty_graph(),
                &settings(),
            );
            assert_eq!(first, again, "same seed must reproduce the identical offer");
        }
    }

    #[test]
    fn different_seeds_can_return_different_offers() {
        let candidates: Vec<TaskSummary> = (1..=30).map(|n| below_task(n, n as i64 * 37)).collect();
        let board = BoardState {
            wip_limit: Some(3),
            current_count: 0,
        };
        let mut distinct = StdHashSet::new();
        for seed in 0..50u64 {
            let outcome = suggest(
                ts(500_000),
                seed,
                &board,
                &candidates,
                &empty_graph(),
                &settings(),
            );
            let Outcome::Offer(offer) = outcome else {
                panic!("expected an Offer")
            };
            let mut ids: Vec<TaskId> = offer
                .items
                .iter()
                .filter_map(|i| match i {
                    OfferItem::Task(t) => Some(t.task_id),
                    OfferItem::Tangle(_) => None,
                })
                .collect();
            ids.sort();
            distinct.insert(ids);
        }
        assert!(
            distinct.len() > 1,
            "50 different seeds over 30 candidates produced only one distinct offer"
        );
    }

    // --- Sampling fairness: every eligible task is eventually offered ---

    #[test]
    fn over_many_seeds_every_eligible_task_is_eventually_offered() {
        // 20 candidates spanning a wide range of `last_touched_at`, so both
        // the recency ("next up") and staleness ("forgotten") weighting are
        // exercised, not just one end of the distribution.
        let candidates: Vec<TaskSummary> =
            (1..=20).map(|n| below_task(n, n as i64 * 1000)).collect();
        let board = BoardState {
            wip_limit: Some(3),
            current_count: 0,
        };
        let mut ever_offered: StdHashSet<TaskId> = StdHashSet::new();
        for seed in 0..1000u64 {
            let outcome = suggest(
                ts(50_000),
                seed,
                &board,
                &candidates,
                &empty_graph(),
                &settings(),
            );
            let Outcome::Offer(offer) = outcome else {
                panic!("expected an Offer")
            };
            for item in offer.items {
                if let OfferItem::Task(t) = item {
                    ever_offered.insert(t.task_id);
                }
            }
        }
        let all_ids: StdHashSet<TaskId> = candidates.iter().map(|c| c.task_id).collect();
        let missing: Vec<TaskId> = all_ids.difference(&ever_offered).copied().collect();
        assert!(
            missing.is_empty(),
            "these eligible tasks were never offered across 1000 seeds: {missing:?}"
        );
    }

    #[test]
    fn top_n_ranking_would_starve_the_middle_but_this_engine_does_not() {
        // The second-oldest task specifically: a top-N "forgotten" slot
        // always picks the single oldest, so if this engine were doing that
        // instead of sampling, the second-oldest would never come up as
        // "forgotten" (though it could in principle appear via "next up" --
        // hence testing the full 1000-seed pool, same as the general
        // fairness test, but calling out this specific task by name).
        let candidates: Vec<TaskSummary> =
            (1..=20).map(|n| below_task(n, n as i64 * 1000)).collect();
        let second_oldest = tid(2); // ages ascend with id, so id 1 is oldest, id 2 second
        let board = BoardState {
            wip_limit: Some(3),
            current_count: 0,
        };
        let mut offered = false;
        for seed in 0..1000u64 {
            let outcome = suggest(
                ts(50_000),
                seed,
                &board,
                &candidates,
                &empty_graph(),
                &settings(),
            );
            let Outcome::Offer(offer) = outcome else {
                panic!("expected an Offer")
            };
            if offer
                .items
                .iter()
                .any(|i| matches!(i, OfferItem::Task(t) if t.task_id == second_oldest))
            {
                offered = true;
                break;
            }
        }
        assert!(
            offered,
            "second-oldest task was never offered across 1000 seeds"
        );
    }

    // --- Bounce/cooldown accounting ---

    #[test]
    fn mark_offered_stamps_last_offered_at() {
        let task = crate::task::create_task(
            tid(1),
            ProjectId::new(Uuid::from_u128(1)),
            "Task",
            "",
            ts(0),
        )
        .unwrap();
        let marked = mark_offered(&task, ts(50));
        assert_eq!(marked.last_offered_at, Some(ts(50)));
    }

    #[test]
    fn bounce_to_below_increments_bounce_count_when_leaving_a_non_done_column() {
        let task = crate::task::create_task(
            tid(1),
            ProjectId::new(Uuid::from_u128(1)),
            "Task",
            "",
            ts(0),
        )
        .unwrap();
        let on_board = crate::task::move_placement(
            &task,
            Placement::OnBoard {
                column: col(1),
                position: 0,
            },
            ts(1),
        )
        .unwrap();
        let bounced = bounce_to_below(&on_board, false, ts(2)).unwrap();
        assert_eq!(bounced.bounce_count, 1);
        assert_eq!(bounced.last_bounced_at, Some(ts(2)));
        assert_eq!(bounced.placement, Placement::Below);
    }

    #[test]
    fn bounce_to_below_does_not_increment_when_leaving_a_done_column() {
        let task = crate::task::create_task(
            tid(1),
            ProjectId::new(Uuid::from_u128(1)),
            "Task",
            "",
            ts(0),
        )
        .unwrap();
        let on_board = crate::task::move_placement(
            &task,
            Placement::OnBoard {
                column: col(1),
                position: 0,
            },
            ts(1),
        )
        .unwrap();
        let moved = bounce_to_below(&on_board, true, ts(2)).unwrap();
        assert_eq!(moved.bounce_count, 0);
        assert_eq!(moved.last_bounced_at, None);
    }

    #[test]
    fn bounce_to_below_accumulates_across_repeated_bounces() {
        let task = crate::task::create_task(
            tid(1),
            ProjectId::new(Uuid::from_u128(1)),
            "Task",
            "",
            ts(0),
        )
        .unwrap();
        let mut current = task;
        for i in 0..3 {
            let on_board = crate::task::move_placement(
                &current,
                Placement::OnBoard {
                    column: col(1),
                    position: 0,
                },
                ts(10 + i),
            )
            .unwrap();
            current = bounce_to_below(&on_board, false, ts(20 + i)).unwrap();
        }
        assert_eq!(current.bounce_count, 3);
    }
}
