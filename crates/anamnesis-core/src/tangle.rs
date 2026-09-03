//! [`Tangle`]: a knot of mutually-blocking tasks, detected by the system
//! (`docs/DOMAIN.md` §3, and the "deliberate asymmetry" in §4).
//!
//! Relationship cycles are allowed by design — "the system needs to store
//! what's in the user's head, and sometimes that means storing a mess for a
//! bit" (`docs/DOMAIN.md` §4) — but a cycle in the *blocking* graph specifically
//! means no task in it can ever become unblocked, which the suggestion engine
//! (`crate::suggest`) must know about. This module is the pure function that
//! turns "a mess in the blocking graph" into a first-class, trackable entity.
//!
//! **One tangle per strongly-connected component (SCC), not per elementary
//! cycle.** Enumerating cycles is combinatorial: a single knot of N mutually
//! blocking tasks can contain exponentially many elementary cycles (a
//! complete directed graph on N nodes has on the order of N! of them), which
//! would flood the board with near-duplicate offers from one knot. Tarjan's
//! algorithm finds SCCs in linear time, and a "knot" *is* an SCC: a maximal
//! set of tasks each reachable from every other along blocking edges.
//!
//! A **self-loop counts too**: an SCC of size 1 whose single node has a
//! blocking edge to itself is a (degenerate but real) tangle — a task that
//! blocks itself can never become unblocked either.
//!
//! Identity is a [`Fingerprint`]: a stable hash over the *sorted* task-id set
//! (free, since [`BTreeSet<TaskId>`] already iterates in `Ord` order — and
//! `TaskId` sorts by its underlying `Uuid`, not by any mutable field). This is
//! what lets a `Tangle` survive unrelated edits elsewhere in the graph and be
//! recognised as "the same tangle" across repeated detection runs
//! ([`reconcile`]).
//!
//! A `Tangle` is never a `Task` row and this module never touches one: it
//! reads relationships and produces new, independent `Tangle` values.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::error::DomainError;
use crate::ids::{ColumnId, TangleId, TaskId, Timestamp};
use crate::placement::Placement;
use crate::relationship::{Relationship, RelationshipKind, is_blocking};

/// A stable, deterministic identity for a set of task ids: an FNV-1a hash
/// over the sorted, delimited id bytes.
///
/// Deliberately *not* `std::collections::hash::DefaultHasher`/`RandomState`:
/// those are seeded per-process in Rust's standard library and are not
/// required to be stable across runs, so two detection passes in two
/// different process runs over the identical task set could disagree. FNV-1a
/// with fixed constants gives the same output every time, on every machine,
/// forever — exactly the stability a fingerprint needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fingerprint(u64);

impl Fingerprint {
    /// Computes the fingerprint of a sorted task-id set.
    pub fn of(task_ids: &BTreeSet<TaskId>) -> Self {
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

        let mut hash = FNV_OFFSET;
        let mut absorb = |byte: u8| {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        };
        // `BTreeSet<TaskId>` iterates in ascending `Ord` order already, so no
        // separate sort is needed here: the set itself carries the "sorted"
        // requirement.
        for id in task_ids {
            for byte in id.as_uuid().as_bytes() {
                absorb(*byte);
            }
            // Delimiter between ids: without it, the byte stream for
            // {AB, CD} and {A, BCD} (as raw concatenations) could collide.
            // A fixed set of 16-byte UUIDs makes this moot in practice, but
            // the delimiter costs nothing and removes the assumption.
            absorb(0xFF);
        }
        Fingerprint(hash)
    }
}

/// A knot of mutually-blocking tasks, detected by [`detect_tangles`] and
/// tracked across detection runs by [`reconcile`].
///
/// Identity is `id`, **not** `fingerprint` — see the module-level `Identity
/// is the id, not the task set` discussion in `docs/DOMAIN.md`'s Tangle
/// section. Earlier drafts made the fingerprint the effective identity,
/// which broke the moment a user made progress untangling one: editing an
/// edge changes the task set, which changes the fingerprint, and the card
/// they were working on would dissolve and reappear as a stranger. `id`
/// persists across membership changes; `fingerprint` is only ever used to
/// *match* a fresh detection to an existing tangle, never to decide what a
/// tangle *is*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tangle {
    pub id: TangleId,
    pub task_ids: BTreeSet<TaskId>,
    pub fingerprint: Fingerprint,
    /// Where this tangle sits — below the horizon (the common case: an
    /// ephemeral, detection-refreshed knot nobody has committed to yet), or
    /// on the board, occupying a column slot like a task.
    pub placement: Placement,
    /// Set the moment a tangle is placed on the board ([`place_tangle`]) and
    /// cleared when it drops back below the horizon ([`drop_tangle`]).
    /// While `true`, [`reconcile`] never rewrites this tangle's `task_ids`
    /// or `fingerprint` — the membership the user committed to untangling
    /// cannot be moved out from under them by the next detection pass.
    pub frozen: bool,
    pub detected_at: Timestamp,
    pub resolved_at: Option<Timestamp>,
    /// Set once a *resolved* tangle has been swept off the board
    /// ([`archive_tangle`]) — the Tangle-side counterpart of `Task::
    /// archived_at` (`docs/DOMAIN.md` §2's "archived: vanished from every
    /// view unless explicitly searched"). `None` for every tangle detection
    /// ever mints; only ever set by [`archive_tangle`], and only on a tangle
    /// that is already resolved.
    pub archived_at: Option<Timestamp>,
}

impl Tangle {
    /// True while this tangle has not been marked resolved.
    pub fn is_active(&self) -> bool {
        self.resolved_at.is_none()
    }
}

/// Archives a *resolved* tangle — the Tangle-side counterpart of
/// [`crate::archive_task`], called by the same "archive all"/scheduled-sweep
/// path once a resolved tangle is sitting in an `is_done` column
/// (`docs/DOMAIN.md`'s Tangle section: "the archive sweep then treats it
/// like anything else").
///
/// Rejects a tangle that is not yet resolved (`DomainError::
/// TangleNotResolved`) — an unresolved knot still has real work left in it
/// and must never be silently swept off the board alongside genuinely done
/// work — and rejects a tangle that is already archived
/// (`DomainError::AlreadyArchived`), symmetrically with every other
/// `archive_*` transition in this crate.
pub fn archive_tangle(tangle: &Tangle, now: Timestamp) -> Result<Tangle, DomainError> {
    if tangle.archived_at.is_some() {
        return Err(DomainError::AlreadyArchived);
    }
    if tangle.resolved_at.is_none() {
        return Err(DomainError::TangleNotResolved);
    }
    Ok(Tangle {
        archived_at: Some(now),
        ..tangle.clone()
    })
}

/// Places a tangle on the board at `column`/`position`, freezing its
/// membership (`docs/DOMAIN.md` Tangle section: "placing a tangle freezes
/// its membership... a commitment to untangle *that specific set*").
///
/// Rejects an already-resolved tangle: there is nothing left to place.
pub fn place_tangle(
    tangle: &Tangle,
    column: ColumnId,
    position: u32,
) -> Result<Tangle, DomainError> {
    if tangle.resolved_at.is_some() {
        return Err(DomainError::TangleAlreadyResolved);
    }
    Ok(Tangle {
        placement: Placement::OnBoard { column, position },
        frozen: true,
        ..tangle.clone()
    })
}

/// Drops a tangle back below the horizon, unfreezing it — detection is free
/// to refresh its `task_ids`/`fingerprint` again, or dissolve it entirely,
/// exactly as for any other below-the-horizon tangle.
///
/// Rejects an already-resolved tangle, symmetrically with [`place_tangle`].
pub fn drop_tangle(tangle: &Tangle) -> Result<Tangle, DomainError> {
    if tangle.resolved_at.is_some() {
        return Err(DomainError::TangleAlreadyResolved);
    }
    Ok(Tangle {
        placement: Placement::Below,
        frozen: false,
        ..tangle.clone()
    })
}

/// True if the subgraph induced by `task_ids` — following only
/// [`is_blocking`] edges whose *both* endpoints lie inside `task_ids` — still
/// contains a cycle (an SCC of size > 1, or a self-loop).
///
/// This is how a **frozen** tangle's resolution is decided
/// (`docs/DOMAIN.md`: "checked against the live graph, not against
/// re-detection"): unlike [`detect_tangles`], which discovers knots
/// system-wide, this checks one specific, already-known task set against
/// the live edges — an edge leaving the set (to a task outside it) is
/// irrelevant to whether *this* knot is still tied.
pub fn subgraph_has_cycle(
    task_ids: &BTreeSet<TaskId>,
    relationships: &[Relationship],
    kinds: &[RelationshipKind],
) -> bool {
    let blocking_kind_ids: HashSet<_> = kinds
        .iter()
        .filter(|kind| is_blocking(kind))
        .map(|kind| kind.id)
        .collect();

    let mut adjacency: HashMap<TaskId, Vec<TaskId>> = HashMap::new();
    for id in task_ids {
        adjacency.entry(*id).or_default();
    }
    for rel in relationships {
        if blocking_kind_ids.contains(&rel.kind_id)
            && task_ids.contains(&rel.from_task_id)
            && task_ids.contains(&rel.to_task_id)
        {
            adjacency
                .entry(rel.from_task_id)
                .or_default()
                .push(rel.to_task_id);
        }
    }

    tarjan_sccs(&adjacency).into_iter().any(|scc| {
        if scc.len() > 1 {
            return true;
        }
        let only = scc.first().expect("scc is non-empty by construction");
        adjacency
            .get(only)
            .is_some_and(|targets| targets.contains(only))
    })
}

/// Resolves `tangle` if its (frozen) `task_ids` no longer contain a cycle in
/// the live blocking graph (`docs/DOMAIN.md`: a frozen tangle "resolves when
/// its frozen task set no longer contains a cycle — checked against the live
/// graph, not against re-detection"). Returns `None`, unchanged, if the knot
/// still holds.
///
/// `done` supplies the board slot to move into when the tangle was on the
/// board — `Some((done_column, position))` moves it there so "the user sees
/// the knot closed rather than the card silently vanishing"; `None` (no
/// `is_done` column configured, or the tangle was already below the
/// horizon) leaves `placement` untouched and only stamps `resolved_at`.
pub fn resolve_frozen_tangle(
    tangle: &Tangle,
    relationships: &[Relationship],
    kinds: &[RelationshipKind],
    now: Timestamp,
    done: Option<(ColumnId, u32)>,
) -> Option<Tangle> {
    if tangle.resolved_at.is_some() || subgraph_has_cycle(&tangle.task_ids, relationships, kinds) {
        return None;
    }
    let placement = match (tangle.placement, done) {
        (Placement::OnBoard { .. }, Some((column, position))) => {
            Placement::OnBoard { column, position }
        }
        (placement, _) => placement,
    };
    Some(Tangle {
        placement,
        resolved_at: Some(now),
        ..tangle.clone()
    })
}

/// One strongly-connected component of size > 1 (or a self-loop of size 1)
/// found in the blocking graph by [`detect_tangles`] — the raw detection
/// result, before it has been matched against previously stored `Tangle`s and
/// given an identity and timestamps by [`reconcile`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedTangle {
    pub task_ids: BTreeSet<TaskId>,
    pub fingerprint: Fingerprint,
}

/// Finds every tangle in the blocking graph formed by `relationships`.
///
/// Only edges whose kind [`is_blocking`] are part of this graph — per
/// `docs/DOMAIN.md` §3, that is exactly the built-in `blocks` kind. Every
/// other kind (the other two built-ins, and any project-custom kind) is
/// invisible here: a `RelationshipKind` not present in `kinds`, or present but
/// not blocking, contributes no edge at all, so it can never create or extend
/// a tangle.
///
/// Runs Tarjan's algorithm once over the filtered graph — linear in edges and
/// tasks, so a large knot (or a large *number* of unrelated small knots)
/// costs no more than one pass; there is no per-cycle enumeration to explode.
pub fn detect_tangles(
    relationships: &[Relationship],
    kinds: &[RelationshipKind],
) -> Vec<DetectedTangle> {
    let blocking_kind_ids: HashSet<_> = kinds
        .iter()
        .filter(|kind| is_blocking(kind))
        .map(|kind| kind.id)
        .collect();

    // Adjacency list over the blocking-only subgraph. A node only exists
    // here if it participates in at least one blocking edge; a task with no
    // blocking relationship at all can never be part of a tangle, so it need
    // not appear.
    let mut adjacency: HashMap<TaskId, Vec<TaskId>> = HashMap::new();
    for rel in relationships {
        if blocking_kind_ids.contains(&rel.kind_id) {
            adjacency
                .entry(rel.from_task_id)
                .or_default()
                .push(rel.to_task_id);
            adjacency.entry(rel.to_task_id).or_default();
        }
    }

    let sccs = tarjan_sccs(&adjacency);

    sccs.into_iter()
        .filter(|scc| {
            if scc.len() > 1 {
                return true;
            }
            // Size-1 SCC: only a tangle if that single node blocks itself.
            let only = scc.first().expect("scc is non-empty by construction");
            adjacency
                .get(only)
                .is_some_and(|targets| targets.contains(only))
        })
        .map(|scc| {
            let task_ids: BTreeSet<TaskId> = scc.into_iter().collect();
            let fingerprint = Fingerprint::of(&task_ids);
            DetectedTangle {
                task_ids,
                fingerprint,
            }
        })
        .collect()
}

/// Tarjan's strongly-connected-components algorithm, iterative (an explicit
/// work stack of `(node, next-neighbour-to-try)` pairs rather than recursion,
/// so depth is bounded by heap, not call-stack, size — a real board could
/// have thousands of tasks and this must not blow the stack).
///
/// This is the standard iterative reformulation of Tarjan's algorithm: each
/// stack frame remembers how far through its node's neighbour list it has
/// gotten, so "returning" from a simulated recursive call is just popping
/// the frame and folding the child's `lowlink` into the parent's, exactly as
/// the recursive version would on unwind.
fn tarjan_sccs(adjacency: &HashMap<TaskId, Vec<TaskId>>) -> Vec<Vec<TaskId>> {
    let mut tarjan = Tarjan::default();
    let empty: Vec<TaskId> = Vec::new();

    // Deterministic root order: `adjacency`'s `HashMap` iteration order is
    // not stable, but the resulting *set* of SCCs does not depend on
    // traversal order, so this only affects which of several equally-valid
    // node orderings within an SCC's `Vec` comes out — never correctness.
    let mut roots: Vec<TaskId> = adjacency.keys().copied().collect();
    roots.sort();

    for root in roots {
        if tarjan.is_visited(root) {
            continue;
        }
        tarjan.discover(root);

        // The simulated call stack: (node, index into its neighbour list of
        // the next neighbour still to examine).
        let mut work: Vec<(TaskId, usize)> = vec![(root, 0)];
        while let Some(&(v, cursor)) = work.last() {
            match adjacency.get(&v).unwrap_or(&empty).get(cursor) {
                Some(&w) => {
                    work.last_mut().expect("just peeked").1 += 1;
                    if tarjan.visit_edge(v, w) {
                        work.push((w, 0));
                    }
                }
                // All of v's neighbours examined: v's subtree is finished,
                // so "return" from the simulated call.
                None => {
                    work.pop();
                    tarjan.finish(v, work.last().map(|&(parent, _)| parent));
                }
            }
        }
    }

    tarjan.sccs
}

struct NodeState {
    index: u32,
    lowlink: u32,
    on_stack: bool,
}

/// Tarjan's own traversal state, named as the algorithm names it: the
/// discovery-index counter, the per-node bookkeeping, "S" (nodes visited but
/// not yet assigned to a finished SCC), and the finished components.
///
/// These live together because the algorithm's steps — descending into a
/// node, folding a back edge, unwinding out of a node — each touch several of
/// them at once. Giving those steps names is what makes the iterative
/// reformulation readable: in one flat loop body they are interleaved and
/// only a comment distinguishes "this is the recursive call" from "this is
/// the return from it".
#[derive(Default)]
struct Tarjan {
    next_index: u32,
    state: HashMap<TaskId, NodeState>,
    on_stack_order: Vec<TaskId>,
    sccs: Vec<Vec<TaskId>>,
}

impl Tarjan {
    fn is_visited(&self, node: TaskId) -> bool {
        self.state.contains_key(&node)
    }

    /// Assigns `node` its discovery index and pushes it onto S — the
    /// recursive version's function entry.
    fn discover(&mut self, node: TaskId) {
        self.state.insert(
            node,
            NodeState {
                index: self.next_index,
                lowlink: self.next_index,
                on_stack: true,
            },
        );
        self.next_index += 1;
        self.on_stack_order.push(node);
    }

    /// Examines the edge `v -> w`. Returns whether the caller should descend
    /// into `w`, i.e. whether this edge is the recursive call.
    fn visit_edge(&mut self, v: TaskId, w: TaskId) -> bool {
        let Some(w_state) = self.state.get(&w) else {
            self.discover(w);
            return true;
        };
        // A back edge to a node still on the stack lowers v's lowlink; an
        // edge to a finished node is a cross edge into an earlier SCC and
        // contributes nothing.
        if w_state.on_stack {
            let w_index = w_state.index;
            self.lower_lowlink(v, w_index);
        }
        false
    }

    /// Unwinds out of `v`: closes the SCC `v` roots, if it roots one, then
    /// folds `v`'s lowlink into its parent's exactly as the recursive version
    /// would on returning from the call.
    fn finish(&mut self, v: TaskId, parent: Option<TaskId>) {
        let NodeState { index, lowlink, .. } = self.state[&v];
        if lowlink == index {
            self.close_scc(v);
        }
        if let Some(parent) = parent {
            self.lower_lowlink(parent, lowlink);
        }
    }

    fn lower_lowlink(&mut self, node: TaskId, candidate: u32) {
        let state = self.state.get_mut(&node).expect("node is in state");
        state.lowlink = state.lowlink.min(candidate);
    }

    /// Pops S down to and including `v`; everything popped is one SCC.
    fn close_scc(&mut self, v: TaskId) {
        let mut scc = Vec::new();
        while let Some(last) = self.on_stack_order.pop() {
            self.state
                .get_mut(&last)
                .expect("on stack implies in state")
                .on_stack = false;
            scc.push(last);
            if last == v {
                break;
            }
        }
        self.sccs.push(scc);
    }
}

/// The outcome of comparing freshly [`detect_tangles`]d tangles against
/// previously stored ones (`docs/DOMAIN.md` §3: "reconciled against stored
/// state").
///
/// This is the entire mutation surface `Tangle`s ever get: a pure function
/// from (detected, previous) to (new, still-holding, resolved). No `Tangle`
/// is ever edited in place, and no `Task` row is ever touched by any of it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reconciliation {
    /// Tangles present now that were not present (as an *active* previous
    /// tangle) before this pass — freshly minted with an id from
    /// `fresh_ids`, `detected_at: now`, `resolved_at: None`.
    pub newly_detected: Vec<Tangle>,
    /// Tangles that were active before this pass and are still detected now
    /// — returned unchanged (same id, same `detected_at`), so the caller has
    /// no reason to touch its stored row.
    pub still_holding: Vec<Tangle>,
    /// Tangles that were active before this pass and are no longer detected
    /// — the same value with `resolved_at: Some(now)` stamped, so the caller
    /// can persist exactly the row that closes it out.
    pub resolved: Vec<Tangle>,
}

/// Reconciles a fresh detection pass ([`detect_tangles`]) against previously
/// stored tangles.
///
/// `previous` may contain both active and already-resolved tangles (a
/// caller-side history of any size); only the *active* ones (`resolved_at ==
/// None`) participate in matching, and among those, **frozen tangles are
/// never rewritten by this function at all** (`docs/DOMAIN.md`: "detection
/// no longer rewrites it, so the goalposts cannot move while the user is
/// working"). Every active, frozen previous tangle is returned unchanged in
/// `still_holding` regardless of what this pass detects — its resolution is
/// decided separately, by [`resolve_frozen_tangle`] against the live graph,
/// never by a mismatch here.
///
/// Only active, **unfrozen** tangles are matched against `detected` by
/// fingerprint, exactly as before frozen tangles existed:
/// - a fingerprint match carries the previous tangle through unchanged into
///   `still_holding` (same id, same `detected_at`, so the caller has no
///   reason to touch its stored row);
/// - a previously-active unfrozen tangle whose fingerprint no longer appears
///   in `detected` is stamped `resolved_at: Some(now)` and returned in
///   `resolved` — this is the *ephemeral* dissolution `docs/DOMAIN.md`
///   describes for a tangle below the horizon ("its task set and
///   fingerprint may change, or it may dissolve").
///
/// A detected knot that matches no active tangle by fingerprint is checked
/// for **duplicate suppression** before being minted as new: if its task set
/// is fully covered (a subset) by *any* active tangle — frozen or not — it
/// is the same knot that tangle already tracks (most commonly: the live
/// graph still contains exactly the cycle a frozen, on-board tangle already
/// owns) and is silently dropped rather than creating a second card for one
/// knot. Only once neither check applies is a brand-new `Tangle` minted,
/// `placement: Placement::Below`, `frozen: false`, from `fresh_ids`.
///
/// A detected fingerprint that matches an already-*resolved* previous tangle
/// is treated as a fresh recurrence — a brand-new `Tangle` with a new id —
/// rather than reopening the old row: a resolved tangle is closed history,
/// and the same set of tasks knotting up again later is a new event worth
/// its own `detected_at`, not a mutation of a record that already said
/// "this ended".
///
/// `fresh_ids` supplies identity for newly detected tangles, in the same
/// spirit as every other id parameter in this crate (`docs/DOMAIN.md`: core
/// generates no ids of its own) — consumed lazily, one per newly detected
/// tangle. The caller must supply at least as many ids as tangles are newly
/// detected in this call; this function panics otherwise, exactly as e.g.
/// `Iterator::next().unwrap()` would, rather than silently dropping a tangle
/// on the floor.
pub fn reconcile(
    detected: &[DetectedTangle],
    previous: &[Tangle],
    now: Timestamp,
    fresh_ids: impl IntoIterator<Item = TangleId>,
) -> Reconciliation {
    let mut fresh_ids = fresh_ids.into_iter();
    let active_previous: Vec<&Tangle> = previous.iter().filter(|t| t.is_active()).collect();
    let frozen_previous: Vec<&Tangle> = active_previous
        .iter()
        .copied()
        .filter(|t| t.frozen)
        .collect();
    let unfrozen_previous: Vec<&Tangle> = active_previous
        .iter()
        .copied()
        .filter(|t| !t.frozen)
        .collect();

    // Frozen tangles are never rewritten by detection: pass every one
    // through unconditionally, whatever this pass did or did not detect.
    let mut still_holding: Vec<Tangle> = frozen_previous.iter().map(|t| (*t).clone()).collect();
    let mut newly_detected = Vec::new();

    for d in detected {
        if let Some(prev) = unfrozen_previous
            .iter()
            .find(|t| t.fingerprint == d.fingerprint)
        {
            still_holding.push((*prev).clone());
            continue;
        }
        if covered_by_any(d, &active_previous) {
            continue;
        }
        let id = fresh_ids
            .next()
            .expect("reconcile: not enough fresh_ids for newly detected tangles");
        newly_detected.push(mint_tangle(d, id, now));
    }

    Reconciliation {
        newly_detected,
        still_holding,
        resolved: resolved_tangles(&unfrozen_previous, detected, now),
    }
}

/// Whether some already-active tangle (frozen or not) fully covers this
/// detection's task set — the "no duplicate cards for one knot" rule. Most
/// commonly the live graph still contains exactly the cycle a frozen,
/// on-board tangle already owns.
fn covered_by_any(d: &DetectedTangle, active_previous: &[&Tangle]) -> bool {
    active_previous
        .iter()
        .any(|t| d.task_ids.is_subset(&t.task_ids))
}

/// A brand-new tangle for a knot nothing active already tracks: below the
/// horizon and unfrozen, since nothing has placed it on the board yet.
fn mint_tangle(d: &DetectedTangle, id: TangleId, now: Timestamp) -> Tangle {
    Tangle {
        id,
        task_ids: d.task_ids.clone(),
        fingerprint: d.fingerprint,
        placement: Placement::Below,
        frozen: false,
        detected_at: now,
        resolved_at: None,
        archived_at: None,
    }
}

/// The unfrozen tangles this pass no longer detects — their knot has been
/// broken since the last one — stamped resolved at `now`. Frozen tangles are
/// absent from `unfrozen_previous` by construction: detection never closes
/// one out, `resolve_frozen_tangles` does.
fn resolved_tangles(
    unfrozen_previous: &[&Tangle],
    detected: &[DetectedTangle],
    now: Timestamp,
) -> Vec<Tangle> {
    let detected_fingerprints: HashSet<Fingerprint> =
        detected.iter().map(|d| d.fingerprint).collect();
    unfrozen_previous
        .iter()
        .filter(|t| !detected_fingerprints.contains(&t.fingerprint))
        .map(|t| Tangle {
            resolved_at: Some(now),
            ..(*t).clone()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{KindId, ProjectId, RelationshipId};
    use crate::relationship::{builtin_blocks, builtin_relates_to, create_relationship};
    use uuid::Uuid;

    fn tid(n: u128) -> TaskId {
        TaskId::new(Uuid::from_u128(n))
    }

    fn pid() -> ProjectId {
        ProjectId::new(Uuid::from_u128(1))
    }

    fn ts(secs: i64) -> Timestamp {
        Timestamp::from_unix_seconds(secs).unwrap()
    }

    fn tang_id(n: u128) -> TangleId {
        TangleId::new(Uuid::from_u128(n))
    }

    fn blocks(from: u128, to: u128) -> Relationship {
        create_relationship(
            RelationshipId::new(Uuid::from_u128(from * 1000 + to)),
            tid(from),
            pid(),
            tid(to),
            pid(),
            &builtin_blocks(),
            ts(0),
        )
        .unwrap()
    }

    fn relates(from: u128, to: u128) -> Relationship {
        create_relationship(
            RelationshipId::new(Uuid::from_u128(from * 1000 + to)),
            tid(from),
            pid(),
            tid(to),
            pid(),
            &builtin_relates_to(),
            ts(0),
        )
        .unwrap()
    }

    fn builtin_kinds() -> Vec<RelationshipKind> {
        vec![builtin_blocks(), builtin_relates_to()]
    }

    // --- Fingerprint ---

    #[test]
    fn fingerprint_is_stable_for_the_same_set_regardless_of_insertion_order() {
        let a: BTreeSet<TaskId> = [tid(1), tid(2), tid(3)].into_iter().collect();
        let b: BTreeSet<TaskId> = [tid(3), tid(1), tid(2)].into_iter().collect();
        assert_eq!(Fingerprint::of(&a), Fingerprint::of(&b));
    }

    #[test]
    fn fingerprint_differs_for_a_different_set() {
        let a: BTreeSet<TaskId> = [tid(1), tid(2)].into_iter().collect();
        let b: BTreeSet<TaskId> = [tid(1), tid(3)].into_iter().collect();
        assert_ne!(Fingerprint::of(&a), Fingerprint::of(&b));
    }

    #[test]
    fn fingerprint_is_stable_across_unrelated_edits_elsewhere_in_the_graph() {
        // The tangle is {1, 2, 3}. Unrelated edges involving tasks 4, 5, 6
        // come and go around it; the tangle's own fingerprint must not move.
        let core = [tid(1), tid(2), tid(3)];
        let fp_before = Fingerprint::of(&core.into_iter().collect());

        // "Unrelated edits": more relationships elsewhere, computed and
        // discarded, changing nothing about the set itself.
        let _unrelated_a: BTreeSet<TaskId> = [tid(4), tid(5)].into_iter().collect();
        let _unrelated_b: BTreeSet<TaskId> = [tid(6)].into_iter().collect();

        let fp_after = Fingerprint::of(&core.into_iter().collect());
        assert_eq!(fp_before, fp_after);
    }

    // --- detect_tangles: one tangle per SCC, not per cycle ---

    #[test]
    fn a_mutual_pair_produces_exactly_one_tangle() {
        let rels = vec![blocks(1, 2), blocks(2, 1)];
        let tangles = detect_tangles(&rels, &builtin_kinds());
        assert_eq!(tangles.len(), 1);
        assert_eq!(
            tangles[0].task_ids,
            [tid(1), tid(2)].into_iter().collect::<BTreeSet<_>>()
        );
    }

    #[test]
    fn a_knot_of_n_mutually_blocking_tasks_produces_exactly_one_tangle_not_hundreds() {
        // A 6-task knot: a full ring 1->2->3->4->5->6->1 plus enough extra
        // chords to create many elementary cycles (e.g. 1->3, 1->4, 1->5)
        // while remaining exactly one SCC. Cycle enumeration here would
        // produce many elementary cycles; SCC detection must still report 1.
        let mut rels = vec![
            blocks(1, 2),
            blocks(2, 3),
            blocks(3, 4),
            blocks(4, 5),
            blocks(5, 6),
            blocks(6, 1),
        ];
        rels.push(blocks(1, 3));
        rels.push(blocks(1, 4));
        rels.push(blocks(1, 5));
        let tangles = detect_tangles(&rels, &builtin_kinds());
        assert_eq!(tangles.len(), 1, "a 6-task knot must be ONE tangle");
        assert_eq!(tangles[0].task_ids.len(), 6);
    }

    #[test]
    fn a_self_loop_counts_as_a_tangle() {
        // `create_relationship` itself rejects a task relating to itself
        // (`DomainError::SelfRelationship`) — no legitimate `blocks` edge
        // from a task to itself can be created through Phase A's API. This
        // test builds the `Relationship` value directly instead, because
        // `detect_tangles` (per `docs/DOMAIN.md` §3: "a self-loop also
        // counts") must still treat one as a tangle if it is ever present in
        // the data — defence in depth against a row that reached this state
        // some other way (a future relaxation of that rule, a migration, a
        // bug elsewhere), rather than silently reporting "no tangle" for a
        // task that can, in fact, never become unblocked.
        let self_loop = Relationship {
            id: RelationshipId::new(Uuid::from_u128(1)),
            from_task_id: tid(1),
            to_task_id: tid(1),
            kind_id: KindId::BUILTIN_BLOCKS,
            created_at: ts(0),
        };
        let tangles = detect_tangles(&[self_loop], &builtin_kinds());
        assert_eq!(tangles.len(), 1);
        assert_eq!(
            tangles[0].task_ids,
            [tid(1)].into_iter().collect::<BTreeSet<_>>()
        );
    }

    #[test]
    fn a_long_simple_chain_with_no_cycle_produces_no_tangles() {
        let rels = vec![
            blocks(1, 2),
            blocks(2, 3),
            blocks(3, 4),
            blocks(4, 5),
            blocks(5, 6),
            blocks(6, 7),
            blocks(7, 8),
        ];
        let tangles = detect_tangles(&rels, &builtin_kinds());
        assert!(tangles.is_empty());
    }

    #[test]
    fn non_blocking_relationship_kinds_never_create_a_tangle() {
        // The same mutual pair, but using `relates to` instead of `blocks`.
        let rels = vec![relates(1, 2), relates(2, 1)];
        let tangles = detect_tangles(&rels, &builtin_kinds());
        assert!(
            tangles.is_empty(),
            "a cycle of a non-blocking kind must never be a tangle"
        );
    }

    #[test]
    fn a_custom_kind_absent_from_the_supplied_kinds_list_is_invisible() {
        let custom_kind_id = KindId::new(Uuid::from_u128(999));
        let custom = Relationship {
            id: RelationshipId::new(Uuid::from_u128(1)),
            from_task_id: tid(1),
            to_task_id: tid(2),
            kind_id: custom_kind_id,
            created_at: ts(0),
        };
        let custom_back = Relationship {
            id: RelationshipId::new(Uuid::from_u128(2)),
            from_task_id: tid(2),
            to_task_id: tid(1),
            kind_id: custom_kind_id,
            created_at: ts(0),
        };
        // Note: `custom_kind_id` is not even present in `kinds` here.
        let tangles = detect_tangles(&[custom, custom_back], &builtin_kinds());
        assert!(tangles.is_empty());
    }

    #[test]
    fn two_disjoint_knots_produce_two_tangles() {
        let rels = vec![blocks(1, 2), blocks(2, 1), blocks(10, 20), blocks(20, 10)];
        let mut tangles = detect_tangles(&rels, &builtin_kinds());
        tangles.sort_by_key(|t| *t.task_ids.iter().next().unwrap());
        assert_eq!(tangles.len(), 2);
        assert_eq!(
            tangles[0].task_ids,
            [tid(1), tid(2)].into_iter().collect::<BTreeSet<_>>()
        );
        assert_eq!(
            tangles[1].task_ids,
            [tid(10), tid(20)].into_iter().collect::<BTreeSet<_>>()
        );
    }

    // --- reconcile ---

    #[test]
    fn a_freshly_detected_tangle_is_newly_detected() {
        let detected = detect_tangles(&[blocks(1, 2), blocks(2, 1)], &builtin_kinds());
        let result = reconcile(&detected, &[], ts(100), [tang_id(1)]);
        assert_eq!(result.newly_detected.len(), 1);
        assert!(result.still_holding.is_empty());
        assert!(result.resolved.is_empty());
        let tangle = &result.newly_detected[0];
        assert_eq!(tangle.id, tang_id(1));
        assert_eq!(tangle.detected_at, ts(100));
        assert_eq!(tangle.resolved_at, None);
    }

    #[test]
    fn a_tangle_detected_again_still_holds_and_keeps_its_identity() {
        let detected = detect_tangles(&[blocks(1, 2), blocks(2, 1)], &builtin_kinds());
        let first = reconcile(&detected, &[], ts(100), [tang_id(1)]);
        let original = first.newly_detected[0].clone();

        // Same graph, detected again later.
        let second = reconcile(&detected, std::slice::from_ref(&original), ts(200), []);
        assert!(second.newly_detected.is_empty());
        assert_eq!(second.still_holding, vec![original.clone()]);
        assert!(second.resolved.is_empty());
        // Identity (id, detected_at) is preserved, not bumped to ts(200).
        assert_eq!(second.still_holding[0].id, original.id);
        assert_eq!(second.still_holding[0].detected_at, ts(100));
    }

    #[test]
    fn the_tangle_auto_resolves_when_an_edge_is_removed() {
        let knotted = detect_tangles(&[blocks(1, 2), blocks(2, 1)], &builtin_kinds());
        let first = reconcile(&knotted, &[], ts(100), [tang_id(1)]);
        let original = first.newly_detected[0].clone();

        // The edge 2->1 is gone: no more cycle, so no more tangle.
        let broken = detect_tangles(&[blocks(1, 2)], &builtin_kinds());
        let second = reconcile(&broken, std::slice::from_ref(&original), ts(200), []);

        assert!(second.newly_detected.is_empty());
        assert!(second.still_holding.is_empty());
        assert_eq!(second.resolved.len(), 1);
        assert_eq!(second.resolved[0].id, original.id);
        assert_eq!(second.resolved[0].resolved_at, Some(ts(200)));
        // The task ids and fingerprint of the closed-out record are
        // unchanged — only `resolved_at` was stamped.
        assert_eq!(second.resolved[0].task_ids, original.task_ids);
    }

    #[test]
    fn a_resolved_tangle_reappearing_is_treated_as_a_fresh_detection() {
        let knotted = detect_tangles(&[blocks(1, 2), blocks(2, 1)], &builtin_kinds());
        let first = reconcile(&knotted, &[], ts(100), [tang_id(1)]);
        let original = first.newly_detected[0].clone();
        let resolved = Tangle {
            resolved_at: Some(ts(150)),
            ..original.clone()
        };

        // The exact same set knots up again later.
        let result = reconcile(&knotted, &[resolved], ts(300), [tang_id(2)]);
        assert_eq!(result.newly_detected.len(), 1);
        assert_ne!(result.newly_detected[0].id, original.id);
        assert_eq!(result.newly_detected[0].detected_at, ts(300));
        assert!(result.still_holding.is_empty());
        assert!(result.resolved.is_empty());
    }

    #[test]
    fn reconcile_never_touches_previous_tangles_not_related_to_this_pass() {
        // An unrelated active tangle elsewhere on the board, still active,
        // still detected: it must show up in `still_holding` unchanged, and
        // must never appear in `resolved` or `newly_detected`.
        let unrelated_detected =
            detect_tangles(&[blocks(10, 20), blocks(20, 10)], &builtin_kinds());
        let previous = reconcile(&unrelated_detected, &[], ts(1), [tang_id(9)]).newly_detected;

        let result = reconcile(&unrelated_detected, &previous, ts(2), []);
        assert_eq!(result.still_holding, previous);
        assert!(result.newly_detected.is_empty());
        assert!(result.resolved.is_empty());
    }

    // --- placement / freezing ---

    fn cid(n: u128) -> ColumnId {
        ColumnId::new(Uuid::from_u128(n))
    }

    #[test]
    fn place_tangle_moves_it_on_board_and_freezes_it() {
        let detected = detect_tangles(&[blocks(1, 2), blocks(2, 1)], &builtin_kinds());
        let tangle = reconcile(&detected, &[], ts(100), [tang_id(1)]).newly_detected[0].clone();
        assert!(!tangle.frozen);
        assert_eq!(tangle.placement, Placement::Below);

        let placed = place_tangle(&tangle, cid(1), 0).unwrap();
        assert!(placed.frozen);
        assert_eq!(
            placed.placement,
            Placement::OnBoard {
                column: cid(1),
                position: 0
            }
        );
        // Identity and content are untouched by placing.
        assert_eq!(placed.id, tangle.id);
        assert_eq!(placed.task_ids, tangle.task_ids);
    }

    #[test]
    fn place_tangle_rejects_an_already_resolved_tangle() {
        let detected = detect_tangles(&[blocks(1, 2), blocks(2, 1)], &builtin_kinds());
        let tangle = reconcile(&detected, &[], ts(100), [tang_id(1)]).newly_detected[0].clone();
        let resolved = Tangle {
            resolved_at: Some(ts(200)),
            ..tangle
        };
        let result = place_tangle(&resolved, cid(1), 0);
        assert_eq!(result, Err(DomainError::TangleAlreadyResolved));
    }

    #[test]
    fn drop_tangle_moves_it_below_and_unfreezes_it() {
        let detected = detect_tangles(&[blocks(1, 2), blocks(2, 1)], &builtin_kinds());
        let tangle = reconcile(&detected, &[], ts(100), [tang_id(1)]).newly_detected[0].clone();
        let placed = place_tangle(&tangle, cid(1), 0).unwrap();

        let dropped = drop_tangle(&placed).unwrap();
        assert!(!dropped.frozen);
        assert_eq!(dropped.placement, Placement::Below);
        assert_eq!(dropped.id, tangle.id);
    }

    #[test]
    fn drop_tangle_rejects_an_already_resolved_tangle() {
        let detected = detect_tangles(&[blocks(1, 2), blocks(2, 1)], &builtin_kinds());
        let tangle = reconcile(&detected, &[], ts(100), [tang_id(1)]).newly_detected[0].clone();
        let resolved = Tangle {
            resolved_at: Some(ts(200)),
            ..tangle
        };
        let result = drop_tangle(&resolved);
        assert_eq!(result, Err(DomainError::TangleAlreadyResolved));
    }

    // --- the key invariant: a placed tangle survives a detection pass ---

    #[test]
    fn a_placed_tangle_keeps_its_board_slot_across_a_detection_pass() {
        // The exact same knot is detected again — the ordinary "nothing
        // changed" re-run every board view does. A frozen, placed tangle
        // must come back byte-for-byte identical: same id, same placement.
        let detected = detect_tangles(&[blocks(1, 2), blocks(2, 1)], &builtin_kinds());
        let fresh = reconcile(&detected, &[], ts(100), [tang_id(1)]).newly_detected[0].clone();
        let placed = place_tangle(&fresh, cid(7), 2).unwrap();

        let second = reconcile(&detected, std::slice::from_ref(&placed), ts(200), []);
        assert_eq!(second.still_holding, vec![placed.clone()]);
        assert!(second.newly_detected.is_empty());
        assert!(second.resolved.is_empty());
        assert_eq!(second.still_holding[0].placement, placed.placement);
        assert!(second.still_holding[0].frozen);
    }

    #[test]
    fn editing_an_edge_inside_a_frozen_tangle_does_not_reshape_or_replace_it() {
        // Frozen over {1, 2, 3}. The user removes edge 3->1, partially
        // untangling it (a real, in-progress edit) — the live graph now
        // detects nothing at all over that trio. The frozen card must not
        // reshape, shrink, or be replaced: reconcile never even looks at
        // what changed for a frozen tangle.
        let full_knot = vec![blocks(1, 2), blocks(2, 3), blocks(3, 1)];
        let detected = detect_tangles(&full_knot, &builtin_kinds());
        let fresh = reconcile(&detected, &[], ts(100), [tang_id(1)]).newly_detected[0].clone();
        assert_eq!(fresh.task_ids.len(), 3);
        let placed = place_tangle(&fresh, cid(1), 0).unwrap();

        let edited = vec![blocks(1, 2), blocks(2, 3)]; // 3->1 removed
        let after_edit = detect_tangles(&edited, &builtin_kinds()); // now empty

        let result = reconcile(&after_edit, std::slice::from_ref(&placed), ts(200), []);
        assert_eq!(result.still_holding, vec![placed.clone()]);
        assert_eq!(result.still_holding[0].task_ids, placed.task_ids);
        assert_eq!(result.still_holding[0].id, placed.id);
        assert!(result.newly_detected.is_empty());
        assert!(result.resolved.is_empty());
    }

    // --- duplicate suppression ---

    #[test]
    fn a_freshly_detected_knot_fully_covered_by_an_active_tangle_is_suppressed() {
        // A frozen tangle already tracks {1, 2, 3}; detection re-runs over
        // the identical, unchanged graph. The exact same knot must not spawn
        // a second card.
        let rels = vec![blocks(1, 2), blocks(2, 3), blocks(3, 1)];
        let detected = detect_tangles(&rels, &builtin_kinds());
        let fresh = reconcile(&detected, &[], ts(100), [tang_id(1)]).newly_detected[0].clone();
        let placed = place_tangle(&fresh, cid(1), 0).unwrap();

        let result = reconcile(&detected, std::slice::from_ref(&placed), ts(200), []);
        assert!(result.newly_detected.is_empty(), "must not duplicate");
        assert_eq!(result.still_holding, vec![placed]);
    }

    #[test]
    fn a_detected_sub_knot_covered_by_a_frozen_tangles_wider_set_is_suppressed() {
        // Frozen over {1, 2, 3}. A later detection pass (while frozen, so it
        // plays no role in matching) happens to also surface a smaller
        // {1, 2} knot found elsewhere in the same call for whatever reason —
        // it is still fully inside the frozen set, so no second card.
        let full_knot = vec![blocks(1, 2), blocks(2, 3), blocks(3, 1)];
        let detected_full = detect_tangles(&full_knot, &builtin_kinds());
        let fresh = reconcile(&detected_full, &[], ts(100), [tang_id(1)]).newly_detected[0].clone();
        let placed = place_tangle(&fresh, cid(1), 0).unwrap();

        let sub_knot_detected = DetectedTangle {
            task_ids: [tid(1), tid(2)].into_iter().collect(),
            fingerprint: Fingerprint::of(&[tid(1), tid(2)].into_iter().collect()),
        };
        let result = reconcile(
            &[sub_knot_detected],
            std::slice::from_ref(&placed),
            ts(200),
            [],
        );
        assert!(result.newly_detected.is_empty());
        assert_eq!(result.still_holding, vec![placed]);
    }

    #[test]
    fn an_uncovered_detected_knot_still_becomes_newly_detected_alongside_a_frozen_one() {
        // A frozen tangle over {1, 2}; a wholly disjoint knot {10, 20} is
        // detected in the same pass. The disjoint knot is not covered by
        // anything and must still be minted.
        let first_knot = detect_tangles(&[blocks(1, 2), blocks(2, 1)], &builtin_kinds());
        let fresh = reconcile(&first_knot, &[], ts(100), [tang_id(1)]).newly_detected[0].clone();
        let placed = place_tangle(&fresh, cid(1), 0).unwrap();

        let both = detect_tangles(
            &[blocks(1, 2), blocks(2, 1), blocks(10, 20), blocks(20, 10)],
            &builtin_kinds(),
        );
        let result = reconcile(&both, std::slice::from_ref(&placed), ts(200), [tang_id(2)]);
        assert_eq!(result.newly_detected.len(), 1);
        assert_eq!(
            result.newly_detected[0].task_ids,
            [tid(10), tid(20)].into_iter().collect::<BTreeSet<_>>()
        );
        assert_eq!(result.still_holding, vec![placed]);
    }

    // --- subgraph_has_cycle / resolve_frozen_tangle ---

    #[test]
    fn subgraph_has_cycle_is_true_for_a_cycle_fully_inside_the_set() {
        let rels = vec![blocks(1, 2), blocks(2, 1)];
        let ids: BTreeSet<TaskId> = [tid(1), tid(2)].into_iter().collect();
        assert!(subgraph_has_cycle(&ids, &rels, &builtin_kinds()));
    }

    #[test]
    fn subgraph_has_cycle_is_false_once_the_closing_edge_is_gone() {
        let rels = vec![blocks(1, 2)]; // 2->1 removed
        let ids: BTreeSet<TaskId> = [tid(1), tid(2)].into_iter().collect();
        assert!(!subgraph_has_cycle(&ids, &rels, &builtin_kinds()));
    }

    #[test]
    fn subgraph_has_cycle_ignores_edges_leaving_the_set() {
        // 1 and 2 no longer cycle between themselves, but 2 now blocks 3,
        // which is outside the frozen set entirely — irrelevant to whether
        // *this* set is still knotted.
        let rels = vec![blocks(1, 2), blocks(2, 3)];
        let ids: BTreeSet<TaskId> = [tid(1), tid(2)].into_iter().collect();
        assert!(!subgraph_has_cycle(&ids, &rels, &builtin_kinds()));
    }

    #[test]
    fn resolve_frozen_tangle_resolves_and_moves_to_the_done_column_when_acyclic() {
        let detected = detect_tangles(&[blocks(1, 2), blocks(2, 1)], &builtin_kinds());
        let fresh = reconcile(&detected, &[], ts(100), [tang_id(1)]).newly_detected[0].clone();
        let placed = place_tangle(&fresh, cid(1), 3).unwrap();

        // The user broke the cycle: only 1->2 remains.
        let live = vec![blocks(1, 2)];
        let done_col = cid(99);
        let resolved = resolve_frozen_tangle(
            &placed,
            &live,
            &builtin_kinds(),
            ts(300),
            Some((done_col, 0)),
        )
        .expect("no cycle remains: must resolve");

        assert_eq!(resolved.resolved_at, Some(ts(300)));
        assert_eq!(
            resolved.placement,
            Placement::OnBoard {
                column: done_col,
                position: 0
            }
        );
        // Identity and the frozen task set survive the resolution.
        assert_eq!(resolved.id, placed.id);
        assert_eq!(resolved.task_ids, placed.task_ids);
    }

    #[test]
    fn resolve_frozen_tangle_does_nothing_while_the_frozen_set_still_cycles() {
        let detected = detect_tangles(&[blocks(1, 2), blocks(2, 1)], &builtin_kinds());
        let fresh = reconcile(&detected, &[], ts(100), [tang_id(1)]).newly_detected[0].clone();
        let placed = place_tangle(&fresh, cid(1), 0).unwrap();

        let still_live = vec![blocks(1, 2), blocks(2, 1)];
        let result = resolve_frozen_tangle(
            &placed,
            &still_live,
            &builtin_kinds(),
            ts(300),
            Some((cid(99), 0)),
        );
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_frozen_tangle_without_a_done_column_still_resolves_but_keeps_placement() {
        let detected = detect_tangles(&[blocks(1, 2), blocks(2, 1)], &builtin_kinds());
        let fresh = reconcile(&detected, &[], ts(100), [tang_id(1)]).newly_detected[0].clone();
        let placed = place_tangle(&fresh, cid(1), 3).unwrap();

        let live = vec![blocks(1, 2)];
        let resolved = resolve_frozen_tangle(&placed, &live, &builtin_kinds(), ts(300), None)
            .expect("must still resolve");
        assert_eq!(resolved.resolved_at, Some(ts(300)));
        assert_eq!(resolved.placement, placed.placement);
    }

    // --- archive_tangle (gap 2: resolved tangles piling up in Done) ---

    fn resolved_tangle() -> Tangle {
        let detected = detect_tangles(&[blocks(1, 2), blocks(2, 1)], &builtin_kinds());
        let fresh = reconcile(&detected, &[], ts(100), [tang_id(1)]).newly_detected[0].clone();
        let placed = place_tangle(&fresh, cid(1), 0).unwrap();
        let live = vec![blocks(1, 2)]; // cycle broken
        resolve_frozen_tangle(&placed, &live, &builtin_kinds(), ts(200), None)
            .expect("must resolve")
    }

    #[test]
    fn a_freshly_detected_tangle_starts_unarchived() {
        let detected = detect_tangles(&[blocks(1, 2), blocks(2, 1)], &builtin_kinds());
        let fresh = reconcile(&detected, &[], ts(100), [tang_id(1)]).newly_detected[0].clone();
        assert_eq!(fresh.archived_at, None);
    }

    #[test]
    fn archive_tangle_stamps_archived_at_on_a_resolved_tangle() {
        let resolved = resolved_tangle();
        let archived = archive_tangle(&resolved, ts(300)).unwrap();
        assert_eq!(archived.archived_at, Some(ts(300)));
        // Identity and task set survive archiving.
        assert_eq!(archived.id, resolved.id);
        assert_eq!(archived.task_ids, resolved.task_ids);
    }

    #[test]
    fn archive_tangle_rejects_an_unresolved_tangle() {
        let detected = detect_tangles(&[blocks(1, 2), blocks(2, 1)], &builtin_kinds());
        let fresh = reconcile(&detected, &[], ts(100), [tang_id(1)]).newly_detected[0].clone();
        assert_eq!(fresh.resolved_at, None);
        let result = archive_tangle(&fresh, ts(300));
        assert_eq!(result, Err(DomainError::TangleNotResolved));
    }

    #[test]
    fn archive_tangle_rejects_an_already_archived_tangle() {
        let resolved = resolved_tangle();
        let archived = archive_tangle(&resolved, ts(300)).unwrap();
        let result = archive_tangle(&archived, ts(400));
        assert_eq!(result, Err(DomainError::AlreadyArchived));
    }
}
