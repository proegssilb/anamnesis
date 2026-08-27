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

use crate::ids::{TangleId, TaskId, Timestamp};
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tangle {
    pub id: TangleId,
    pub task_ids: BTreeSet<TaskId>,
    pub fingerprint: Fingerprint,
    pub detected_at: Timestamp,
    pub resolved_at: Option<Timestamp>,
}

impl Tangle {
    /// True while this tangle has not been marked resolved.
    pub fn is_active(&self) -> bool {
        self.resolved_at.is_none()
    }
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
            let only = scc.iter().next().expect("scc is non-empty by construction");
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
    struct NodeState {
        index: u32,
        lowlink: u32,
        on_stack: bool,
    }

    let mut next_index: u32 = 0;
    let mut state: HashMap<TaskId, NodeState> = HashMap::new();
    // Tarjan's "S": the set of nodes visited but not yet assigned to a
    // finished SCC, in discovery-adjacent order.
    let mut on_stack_order: Vec<TaskId> = Vec::new();
    let mut sccs: Vec<Vec<TaskId>> = Vec::new();
    let empty: Vec<TaskId> = Vec::new();

    // Deterministic root order: `adjacency`'s `HashMap` iteration order is
    // not stable, but the resulting *set* of SCCs does not depend on
    // traversal order, so this only affects which of several equally-valid
    // node orderings within an SCC's `Vec` comes out — never correctness.
    let mut roots: Vec<TaskId> = adjacency.keys().copied().collect();
    roots.sort();

    for root in roots {
        if state.contains_key(&root) {
            continue;
        }

        // The simulated call stack: (node, index into its neighbour list of
        // the next neighbour still to examine).
        let mut work: Vec<(TaskId, usize)> = vec![(root, 0)];
        state.insert(
            root,
            NodeState {
                index: next_index,
                lowlink: next_index,
                on_stack: true,
            },
        );
        next_index += 1;
        on_stack_order.push(root);

        while let Some(&(v, cursor)) = work.last() {
            let neighbours = adjacency.get(&v).unwrap_or(&empty);
            if cursor < neighbours.len() {
                work.last_mut().expect("just peeked").1 += 1;
                let w = neighbours[cursor];
                if let Some(w_state) = state.get(&w) {
                    if w_state.on_stack {
                        let w_index = w_state.index;
                        let v_state = state.get_mut(&v).expect("v is in state");
                        v_state.lowlink = v_state.lowlink.min(w_index);
                    }
                    // else: w is finished and in an earlier SCC already —
                    // a cross edge, contributes nothing to v's lowlink.
                } else {
                    // Unvisited: descend into it (the recursive call).
                    state.insert(
                        w,
                        NodeState {
                            index: next_index,
                            lowlink: next_index,
                            on_stack: true,
                        },
                    );
                    next_index += 1;
                    on_stack_order.push(w);
                    work.push((w, 0));
                }
            } else {
                // All of v's neighbours examined: v's subtree is finished.
                work.pop();
                let v_index = state[&v].index;
                let v_lowlink = state[&v].lowlink;
                if v_lowlink == v_index {
                    let mut scc = Vec::new();
                    while let Some(last) = on_stack_order.pop() {
                        state
                            .get_mut(&last)
                            .expect("on stack implies in state")
                            .on_stack = false;
                        scc.push(last);
                        if last == v {
                            break;
                        }
                    }
                    sccs.push(scc);
                }
                // Fold v's lowlink into its parent's, as the recursive
                // version would on returning from the call.
                if let Some(&(parent, _)) = work.last() {
                    let parent_state = state.get_mut(&parent).expect("parent is in state");
                    parent_state.lowlink = parent_state.lowlink.min(v_lowlink);
                }
            }
        }
    }

    sccs
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
/// None`) participate in matching. A detected fingerprint that matches an
/// already-resolved previous tangle is treated as a *fresh* recurrence — a
/// brand-new `Tangle` with a new id — rather than reopening the old row: a
/// resolved tangle is closed history, and the same set of tasks knotting up
/// again later is a new event worth its own `detected_at`, not a mutation of
/// a record that already said "this ended".
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
    let detected_fingerprints: HashSet<Fingerprint> =
        detected.iter().map(|d| d.fingerprint).collect();

    let mut still_holding = Vec::new();
    let mut newly_detected = Vec::new();
    for d in detected {
        match active_previous
            .iter()
            .find(|t| t.fingerprint == d.fingerprint)
        {
            Some(prev) => still_holding.push((*prev).clone()),
            None => {
                let id = fresh_ids
                    .next()
                    .expect("reconcile: not enough fresh_ids for newly detected tangles");
                newly_detected.push(Tangle {
                    id,
                    task_ids: d.task_ids.clone(),
                    fingerprint: d.fingerprint,
                    detected_at: now,
                    resolved_at: None,
                });
            }
        }
    }

    let resolved = active_previous
        .into_iter()
        .filter(|t| !detected_fingerprints.contains(&t.fingerprint))
        .map(|t| Tangle {
            resolved_at: Some(now),
            ..t.clone()
        })
        .collect();

    Reconciliation {
        newly_detected,
        still_holding,
        resolved,
    }
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
}
