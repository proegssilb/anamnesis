# Anamnesis — Real Domain Model

> Approved design. Supersedes the placeholder model in `ARCHITECTURE.md`,
> which describes the disposable kanban scaffold and is now historical.

## Context

The placeholder kanban proved the stack: four crates with a pure core, dual
SQLite/Postgres persistence, provider-agnostic OIDC, no-JS server-rendered UI,
129 tests + 19 BDD scenarios, all green and verified running end to end.

Its domain model was always disposable, and it has now hit exactly the wall
predicted in `docs/ARCHITECTURE.md`: `Board` is the consistency boundary, but
task relationships cross projects, so **no single aggregate can own the graph.**
This replaces the domain model wholesale and reshapes persistence to match.

The product premise governs every decision below: **let the user actually forget
things.** Dump a task in, trust the system to resurface it at the right moment,
stop carrying it in your head. Capture friction must stay near zero; resurfacing
must stay gentle and capacity-gated.

---

## 1. Weight: resolved — no weight field

**One task = one notch against the WIP limit.** No size/estimate field exists.

An estimate is a guess made at capture time, when you know least; it decays
silently as the task changes; and it adds friction to the one operation that
must stay frictionless. The information is already legible in the title
("Regrout the shower" vs "Order grout") faster than any field could convey it.
Estimation is also the specific thing that makes work tooling miserable, and
this system exists to not do that.

Weight is instead **observed, never asked**, from data captured for other
reasons:

| Signal | Source | Cost |
|---|---|---|
| Checklist size | count of child tasks | free, structural |
| Staleness | `last_touched_at` | free, already needed for "forgotten" |
| **Bounce count** | times raised above the horizon then set back down unfinished | free, behavioural |

**Bounce count is the important one.** It measures actual resistance rather than
estimated size. It does not size the task; it changes *how the system asks*. A
repeatedly-bounced task gets a different prompt ("this one keeps coming back —
want to break it up, or let it go?"), turning resurfacing into something that
helps the user notice avoidance rather than nagging them.

Vocabulary: **"a haul"** is the informal word for a heavy task, used in prompt
copy only. It is not a stored attribute.

---

## 2. Core metaphor: the horizon

> "What's below the horizon isn't gone — it's just not up yet."

The tagline is the model. Every task is in exactly one of three placements:

```rust
enum Placement {
    Below,                                        // backlog. Out of sight, zero load.
    OnBoard { column: ColumnId, position: u32 },  // visible on the task board
}
// plus, orthogonally: archived_at: Option<Timestamp>  — gone unless searched
```

- **Below the horizon** — exists, keeps its relationships, costs nothing. Where
  most tasks live most of the time.
- **Above** — on the global task board, in a column. Column *is* status.
- **Archived** — vanished from every view unless explicitly searched.

This resolves "status = the column it's in": `Below` is the backlog state, so
the WIP limit has something real to gate, and the suggestion engine has a
well-defined job — **it is the mechanism that raises things above the horizon.**

---

## 3. Entities

### Area
Parallel domains of life; the masks a person wears. Displayed as a grid.

`id, title, description, position, created_at, updated_at`

### Project
Concrete sagas or ongoing commitments. Owns its own task vocabulary.

`id, area_id, title, description, status: Pending | Active | Complete,
created_at, updated_at, archived_at`

Owns (loaded with it — small, config-like): `FieldDefinition[]`,
`RelationshipKind[]`.

Global invariant: `count(status == Active) <= settings.active_project_limit`.

### Task
`id, project_id, title, description, placement, parent_task_id: Option<TaskId>,
checklist_position, created_at, last_touched_at, archived_at,
bounce_count, last_bounced_at, last_offered_at`

Plus `FieldValue[]` (loaded with the task). Comments and attachments load
separately — append-heavy, rarely all needed at once.

**Checklists are containment**: a task's checklist items *are* tasks, via
`parent_task_id`. A checklist item can be raised above the horizon
independently of its parent — containment and placement are orthogonal.

### Relationship (a standalone edge)
`id, from_task_id, to_task_id, kind_id, created_at`

Edges live **outside** projects: any task may relate to any task across areas
and projects, because real blockers cross domains constantly. This is precisely
why `Board` cannot remain the aggregate.

### RelationshipKind
`id, project_id: Option<ProjectId>, forward_label, reverse_label`

- `project_id: None` → **built-in**, available everywhere.
- `project_id: Some(_)` → project-local custom vocabulary.
- **Cross-project edges may only use built-in kinds** — a custom kind belongs to
  one project, and using it on an edge whose far end lives elsewhere leaves its
  ownership and visibility ambiguous.

**Only the built-in `blocks` / `blocked by` kind gates availability.** Custom
kinds are labels — vocabulary for how *you* describe a link ("inspired by",
"same shop trip", "see also") — and carry no scheduling meaning. This keeps the
suggestion engine and tangle detection reading one well-defined edge type
instead of guessing intent from free text. If a project ever genuinely needs a
second blocking kind, adding a boolean to this table is a trivial migration;
do not build it now.

Built-ins: `blocks / blocked by` (the only blocking kind), `relates to`,
`duplicates / duplicated by`.

### FieldDefinition / FieldValue
`FieldDefinition: id, project_id, name, kind, position, show_on_card: bool`

```rust
enum FieldKind { Number, Currency, Date, Time, DateTime, Line, Block }
```

`show_on_card` drives compact card rendering — some fields deliberately do not
appear on the board. Values stored per task in a typed EAV table (separate
`value_int / value_text / value_ts` columns, not JSON, so both backends sort
and filter natively).

**Currency stores integer minor units + ISO 4217 code — never a float.**
Number stores a scaled integer (value + scale) for the same reason.

### Comment / Attachment
`Comment: id, task_id, author, body, created_at, edited_at`

`Attachment: id, task_id, kind: Link { url } | File { blob_key, filename, mime, size }`

Files need a new `BlobStore` port (local filesystem first, S3-shaped later).

### Tangle
A knot of mutually-blocking tasks, detected by the system.

`id, task_ids: Set<TaskId>, fingerprint, detected_at, resolved_at: Option<_>`

**One tangle per strongly-connected component** of the blocking graph, not per
distinct cycle. Tarjan's algorithm is linear; enumerating elementary cycles is
combinatorial and would flood the board with near-duplicates from one knot.
Identity is a fingerprint over the sorted task-id set, so a tangle survives
unrelated edits and auto-resolves when the cluster breaks up.

A Tangle is **its own entity rendered as a card, not a Task row** — the system
never creates, deletes or mutates rows in the user's own task table, detection
stays a pure function (`graph → Set<Tangle>`) reconciled against stored state,
and a tangle never inherits an edit surface that makes no sense (reassigning it
to a project, editing its title).

**Resolving a tangle is itself a suggestable item.** Tangled tasks are excluded
from suggestions (a knotted task is not actionable), but the tangle is offered
in their place — "these four are knotted, want to untie them?". Accepting it
puts it on the board where it costs a notch like any other work, because
untangling *is* work. Tangles also remain visible as a quiet indicator so they
are discoverable without waiting for an offer.

### Column (global, task board)
`id, title, position, wip_limit: Option<u32>, is_done: bool`

Defaults: **To-Do** (WIP-limited), **Doing**, **Done**. Columns are global — the
task board spans all active projects and areas, and its WIP limits apply across
all of them.

The **project board** is separate, with fixed columns
(Pending / Active / Complete) derived from `Project.status`.

### Settings & People
`Settings: active_project_limit, timezone, sweep_recurrence, suggestion config`

Timezone is required once scheduled sweeps exist (§6) — "every other Monday"
is meaningless without one.

Roles: **System Admin** (users, global settings, columns, limits),
**Project Admin** (a project's fields, kinds, membership), **Member**.
`can_view` generalises into a `policy` module in core.

---

## 4. Deliberate asymmetry: which cycles are allowed

| Graph | Cycles | Why |
|---|---|---|
| **Relationships** | **Allowed** | "The system needs to store what's in the user's head, and sometimes that means storing a mess for a bit." Detected, surfaced as a Tangle, never rejected. |
| **Containment** (checklists) | **Rejected** | A task containing its own ancestor breaks rendering and roll-up with no user meaning. Enforced acyclic in core. |

Intentional, and worth stating in the code — it looks inconsistent otherwise.

---

## 5. The suggestion engine — the soul of the product

A **pure function** in core. Highest-value thing to get right, and the easiest
to test, because it is total and deterministic given `now` and `seed`.

```rust
pub fn suggest(
    now: Timestamp,
    seed: u64,                  // entropy is an INPUT — core stays pure
    board: &BoardState,
    candidates: &[TaskSummary],
    graph: &BlockingGraph,
    settings: &SuggestionSettings,
) -> Outcome
```

### Outcome — and the rule that the system never sasses

```rust
enum Outcome {
    Full,                 // board at limit → SAY NOTHING AT ALL
    Offer(Offer),         // space + something eligible → offer up to 3
    Stuck(Blockage),      // space, but nothing eligible → explain why
}
```

**`Full` is silence, and that is a feature.** A full board means the user is
already carrying as much as they agreed to carry. They are probably having a
hell of a week and do not need their task tracker piping up. No message, no
banner, no nudge — the system simply has nothing to say and says nothing.

**`Stuck` is the `Err` arm**: it explains why the engine produced no
suggestions **for a board that has room.** That is the only case where silence
would read as a broken app, because the user can see the empty slot and
reasonably expects something to fill it. Blockage reasons are concrete and
actionable:

- everything eligible is blocked by unfinished work
- everything eligible is knotted in a tangle
- no project is Active
- everything has been offered recently and is still on cooldown
- the backlog is empty

### Eligibility (all must hold)
- not archived
- `placement == Below`
- owning project `status == Active`
- **unblocked**: no incoming `blocks` edge from a task that is not Done
- **not in an unresolved Tangle** — a knotted task is not actionable
- **off cooldown**: `last_offered_at` older than the decline cooldown, so
  declining something does not immediately re-offer it

### Composition — weighted sampling, not ranking

Offer size = `min(3, wip_limit - current_count)`.

- **2 × "next up"** — sampled from a **recency-weighted** distribution
- **1 × "forgotten"** — sampled from a **staleness-weighted** distribution over
  the older tail

**Sampling, not top-N, is a correctness requirement.** Deterministic ranking
starves the middle of the distribution: `last_touched_at` only updates when the
user interacts, so a task never offered is never touched, so it never becomes
"most recent" — and a top-N "forgotten" slot always picks the single oldest,
leaving second-oldest waiting indefinitely. Weighted sampling guarantees every
eligible task has non-zero probability, so the net widens over time.

**Seed stability matters as much as randomness.** Derive `seed` from
`(user, local date, board-state fingerprint)` rather than fresh entropy, so the
same three suggestions persist across a page refresh and change when the
situation changes. Re-rolling on every F5 would let the user slot-machine for
an easy task, which defeats the gentle-nudge intent.

Bounce accounting: moving a task `OnBoard → Below` without reaching an
`is_done` column increments `bounce_count` and stamps `last_bounced_at`.
Every offer stamps `last_offered_at`.

---

## 6. Recurrence and sweeping

The first genuinely time-triggered behaviour in the system.

```rust
enum Recurrence {
    EveryNWeeks { n: u8, weekday: Weekday },  // "every other Monday"
    DayOfMonth { day: u8 },                   // "the 15th"
    Never,
}
```

Two pure functions, no I/O: `next_run(recurrence, from, tz) -> Timestamp` and
`sweep_done(tasks, now) -> Vec<TaskId>`. The shell owns a ticker that asks
whether a sweep is due and applies the result. Timezone comes from settings.

- **Archive day**: on schedule, everything in a `is_done` column is archived.
- **"Archive all" button**: always available on any list representing
  completion (the Done column, a completed-project list), independent of
  whether a schedule is configured. The manual path must work even if the
  scheduled one never fires.

`Recurrence` is deliberately a reusable value type — recurring *tasks* are an
obvious future feature — but **recurring tasks are not in scope now.**

---

## 7. Architectural consequences

**Whole-aggregate save dies here.** Correct for one self-contained board, wrong
for a global graph.

| Aggregate | Loaded with | Notes |
|---|---|---|
| `Area` | — | tiny |
| `Project` | field definitions, relationship kinds | config-sized |
| `Task` | field values | comments/attachments paged separately |
| `Relationship` | — | standalone edge rows |
| `Tangle` | — | system-derived, reconciled |
| `Comment`, `Attachment` | — | append-heavy, paged |

Repository ports become **per-entity with targeted operations**. Add an
`updated_at`-based optimistic concurrency check on `Task` — with finer-grained
edits and plausible multi-device use, last-write-wins is no longer acceptable.

**Introduce read models (CQRS-lite).** The task board is a *query* across
everything above the horizon grouped by column — not an aggregate. Separate
`BoardQuery` / `SearchQuery` ports keep board rendering from loading object
graphs. This is the single most important structural addition.

**New ports:** `BlobStore` (attachments), `SearchIndex` (global search), `Clock`
already exists. Search is the largest new backend divergence — SQLite FTS5 vs
Postgres `tsvector`/GIN — and must sit behind the port with one shared contract
test, exactly as the board repository does today.

**Survives unchanged:** crate boundaries, ports pattern, four-layer test
strategy, OIDC, config, session/CSRF, dual-backend adapter approach, MiniJinja.
**Replaced:** everything in `anamnesis-core`, the `BoardRepository` port, the
SQL schema, most templates.

---

## 8. UI

**htmx + `hx-trigger`**, replacing the no-JS constraint (owner's call).

**On needing both htmx and a drag library:** they do different jobs and neither
covers the other. htmx has no drag support at all — it fires requests on
events. A drag library supplies the pointer/touch mechanics and the live
"card follows your finger" feedback; htmx supplies the transport and swaps the
returned fragment. The division is: **Sortable drags, htmx persists.**

The alternative — native HTML5 drag-and-drop, no library — is rejected on one
specific ground: it is unreliable on touch devices, and a mobile PWA is a
stated goal. Desktop-only dragging would not survive the thing this is being
built toward.

You could also drop htmx *from the drag path* and let Sortable's `onEnd` call
`fetch()` directly. Not recommended: it splits the app into two transport
stories, and the server would need a JSON path alongside the fragment path for
no gain.

**Reduce the surface instead:** only *cards* need dragging. Custom-field
ordering is rare, admin-shaped, and served perfectly well by up/down buttons —
which are plain form posts, need no library, and work on touch by default.
Ship drag for the board; skip it for field config unless it proves annoying.

- Endpoints stay resource-oriented — they already are, so no reshaping needed.
- Templates become **fragment-addressable** (one card, one column) so htmx can
  swap precisely.
- **Keep the form-POST fallbacks.** They work, they keep the app usable without
  JS, and they preserve the PWA-ready route shape.
- No real-time collaboration, no server push — **a refresh is the update
  mechanism.** Out of scope by decision, not omission.

Views: task board (global), project board, area grid,
project-as-flat-list, task detail (fields, relationships, checklist, comments,
attachments), global search across areas + projects + tasks.

---

## 9. Open questions (non-blocking; assumptions stated)

1. **Number precision.** Assumed scaled integers. Arbitrary precision would
   need a decimal crate.
2. **Cooldown length.** Assumed a few days for declined suggestions;
   configurable, tune once it is in use.
3. **Sampling weights.** Assumed a simple staleness curve; the exact shape is
   worth tuning against real data rather than guessing now.

---

## 10. Execution plan

Design doc first for approval. Then the vertical slice, delegated aggressively
to subagents, TDD throughout, one phase per agent, sequential. Every phase:
tests first, `fmt` + `clippy -D warnings` clean, committed, independently
verified before the next starts.

| Phase | Scope | Verification |
|---|---|---|
| **A. Core model** | Areas, Projects, Tasks, placement, containment (acyclic), fields, relationships, kinds | Unit tests; containment-cycle rejection; cross-project edge rules |
| **B. Tangles + suggestions** | Tarjan SCC, fingerprint identity, reconciliation; `suggest` with seeded sampling; `Outcome::Stuck`; bounce + cooldown accounting | Unit + BDD. Both pure — heavy coverage: knot→tangle→resolve, tangle offered in place of its tasks, WIP gating, `Stuck` diagnostic, sampling fairness over many seeds, seed stability across refresh |
| **C. Recurrence + sweep** | `Recurrence`, `next_run`, `sweep_done`, timezone handling, manual archive-all | Pure unit tests incl. DST and month-end edges |
| **D. Ports + use cases** | Per-entity repositories, `BoardQuery`, `SearchQuery`, `BlobStore`, `SearchIndex`; use cases; policy/roles | BDD against in-memory fakes; authorization per role |
| **E. Persistence** | New schema both backends, migrations, targeted updates, optimistic concurrency, FTS5 + tsvector | One shared contract test across both backends; Postgres job in CI |
| **F. Web + htmx** | Fragment templates, drag-drop, area grid, both boards, tangle indicator, task detail, search, suggestion prompt | `oneshot` integration tests; **run the app and drive it** |
| **G. Docs** | Rewrite `ARCHITECTURE.md` + `CONTEXT.md`; migration notes | README quickstart re-verified from clean |

Phase B is the highest-value work and gets the most scenario coverage — it is
the product's reason to exist, and it is pure, so there is no excuse for thin
tests.

---

## 11. Verification

- `cargo test --workspace` green; `fmt --check` and `clippy -D warnings` clean.
- The Postgres contract test runs for real (locally and in CI), not skipped.
- BDD scenarios read as behaviour the owner recognises — especially: a knot
  becomes one tangle and auto-resolves; the tangle is offered in place of its
  knotted tasks; the engine stays quiet at WIP limit but explains itself when
  nothing is actionable; a bounced task gets the softer prompt; the same seed
  returns the same three suggestions.
- **Run the app and drive it**: area → project → tasks, tangle two tasks and
  watch it surface and resolve, accept an untangle suggestion, drag a card
  between columns, accept a task suggestion, run an archive-all.
- Kill every server started during verification; leave no stray processes.
