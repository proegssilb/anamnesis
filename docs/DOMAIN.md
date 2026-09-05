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

Files need a new `BlobStore` port (local filesystem or an S3-compatible
object store, chosen per deployment).

### Tangle
A knot of mutually-blocking tasks, detected by the system.

```rust
Tangle {
    id: TangleId,              // STABLE IDENTITY — survives membership changes
    task_ids: Set<TaskId>,     // content, not identity
    fingerprint: Fingerprint,  // content hash, for matching detections
    placement: Placement,      // tangles sit on the board like tasks
    frozen: bool,              // set when placed; detection stops rewriting it
    detected_at: Timestamp,
    resolved_at: Option<Timestamp>,
}
```

**One tangle per strongly-connected component** of the blocking graph, not per
distinct cycle. Tarjan's algorithm is linear; enumerating elementary cycles is
combinatorial and would flood the board with near-duplicates from one knot.

A Tangle is **its own entity, not a Task row.** That is precisely what makes the
rest of this safe: the system creates, updates and deletes tangles freely
without ever touching the user's own tasks table, and detection stays a pure
function (`graph → Set<DetectedTangle>`) reconciled against stored state.

**Untangling is work, so a tangle can be placed on the board** — raised above
the horizon, occupying a column slot and counting against that column's WIP
limit exactly like a task. It is offered by the suggestion engine in place of
its knotted member tasks (which are themselves never offered, being
unactionable), and accepting the offer places it.

#### Identity is the id, not the task set

Earlier drafts made the fingerprint the effective identity, which broke under
use: untangling *is* edge-editing, so the moment you make progress the task set
changes, the fingerprint changes, and the card you were working on would
dissolve and reappear as a stranger. Hence:

- **`TangleId` is the identity** and persists across membership changes.
  `fingerprint` is a content hash used to match a fresh detection to an
  existing tangle — never to decide what a tangle *is*.
- **A tangle below the horizon is ephemeral.** Detection refreshes it freely;
  its task set and fingerprint may change, or it may dissolve.
- **Placing a tangle freezes its membership.** A placed tangle is a commitment
  to untangle *that specific set*. Detection no longer rewrites it, so the
  goalposts cannot move while the user is working.
- **A frozen tangle resolves when its frozen task set no longer contains a
  cycle** — checked against the live graph, not against re-detection. On
  resolving while on the board it moves to the `is_done` column, so the user
  sees the knot closed rather than the card silently vanishing; the archive
  sweep then treats it like anything else.
- **No duplicate cards for one knot**: a freshly detected knot already fully
  covered by an active tangle is suppressed rather than creating a second.

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

---

## 12. Decisions made during implementation

This design was approved before a line of the real model was written. A
handful of things resolved only once the model was actually built and
tested — recorded here, with the reasoning, because the reasoning is what
keeps a future change from reintroducing the same defect. (`docs/ARCHITECTURE.md`
covers the same ground from the systems/code-structure angle; this section
is the *why*, kept next to the design it amends.)

### Area-scoped roles, and composition by strongest grant

§3 named three roles but scoped them only to projects. Two problems showed
up once membership was actually implemented: a purely area-level action
(view the area, manage it) had nowhere to hang except System Admin, and
`CreateProject` was chicken-and-egg — a project that does not exist yet has
no project role to check. Fix: **Areas are a real membership scope**, and a
Project inherits its Area's role when it carries no explicit role of its
own.

Inheritance alone was not sufficient, and the first version of it was wrong
in a way worth stating plainly. It composed scopes by "most specific wins" —
an explicit project-level grant overrode the inherited Area grant outright,
even when the explicit grant was *weaker*. That silently demoted a System
Admin (or an Area Admin) who also happened to hold a plain Member row on one
project — precisely backwards, since adding someone to a project was meant
to add capability, not remove it. The corrected rule is **composition by
strongest grant**: System Admin status, the Area grant, and the Project
grant are three independent grants, and the effective role is the strongest
of the three. This is the same property `chmod` gets right — granting a
permission bit never revokes one already held — and it is a *monotonicity*
property, so it was tested as one rather than as a handful of examples: a
property test enumerates every `(project_role, area_role, is_system_admin)`
starting state and every way to strengthen exactly one of those three slots,
and asserts the effective role never goes down. That test is what actually
caught the System-Admin-demotion defect above; the individual before/after
example tests in the same file would have kept passing under "most specific
wins" for the particular cases they happened to construct.

### Identity-provider groups as a fourth grant

Every grant above is keyed on a single user id, which means onboarding
anyone starts with learning their `sub`. Deployments whose identity provider
already models access as groups can optionally add a **fourth independent
grant**: a group named by the configured groups claim can hold System Admin,
or a role on an Area or a Project, exactly as a user can.

"Independent" is the whole point, and it is the same rule as above rather
than a new one: the effective role is the strongest of *four* grants now —
System Admin, the Area grant, the Project grant, and whatever the user's
groups hold at any of those scopes. A group grant can never demote someone,
and never overrides a grant held directly. It composes by `.max()` in one
place (`anamnesis_app::access`), and the per-user resolution it composes
over is unchanged.

Two asymmetries with the per-user path are deliberate:

**Group membership is a cached provider fact; mappings are ours.** The
groups a user presented are recorded at login and re-read only at the next
one (no refresh token is stored), while the group→role mappings live in the
database and are joined at request time. So revoking a *mapping* takes
effect immediately, whereas someone removed from a group in the provider
keeps what it granted until they sign in again.

**Unmapping an admin group has no last-admin check**, unlike revoking System
Admin from the last user who holds it. That check exists to prevent locking
everyone out, and it can answer the question because `system_admins` is an
exhaustive list of who holds admin. A count over admin *groups* cannot: a
mapped group is not evidence that any user is in it, since Anamnesis never
enumerates the provider's directory. Rather than a check that looks like a
guarantee and isn't, there is none — and the per-user check remains the real
lockout guard.

### Tangle identity is `TangleId`, not the fingerprint

Covered in full in §3 ("Identity is the id, not the task set") — recorded
here only as an index entry, since it is exactly this kind of
implementation-time correction. Summary: the fingerprint was originally the
effective identity, which broke the moment untangling was tried in
practice — untangling *is* edge-editing, so the task set (and hence the
fingerprint) changes as soon as real progress happens, and a
fingerprint-identified card dissolves and reappears as a stranger mid-work.
`TangleId` is now the stable identity; `fingerprint` is only ever used to
match a fresh detection pass against an existing tangle, never to decide
what a tangle *is*. Membership freezes the moment a tangle is placed on the
board, so the goalposts cannot move while the user is working it.

### The `tangled_task_ids` / `tangles` split

`anamnesis_core::BlockingGraph` (the suggestion engine's view of blocking
and tangling) carries two related but independent fields rather than one
derived from the other: `tangled_task_ids` (every task id currently bound up
in an unresolved tangle, full stop) and `tangles` (unresolved tangles
currently *offerable* in place of their members). Ordinarily every id in the
first traces back to a tangle in the second — but not always: a tangle
already accepted onto the board occupies its own slot as a work item and
must not be offered a second time, while its member tasks must stay excluded
from individual suggestion until the knot actually resolves. Collapsing
these into one field (derive "excluded tasks" from "offerable tangles," or
vice versa) cannot express that state, and without it `Blockage::AllTangled`
— "everything eligible is knotted, and unusually no tangle is available to
offer in its place" — has no way to become reachable at all: the tangle
would always still be sitting there as an offerable substitute. The split
makes it possible for a placed tangle to correctly suppress its own members
from suggestion while itself no longer counting as something new to offer.

### Offer composition below three free slots, and `Blockage` precedence

§5 specifies a *full* offer as 2 "next up" + 1 "forgotten," but a board with
only 1 or 2 free slots was left unresolved. Resolved by shrinking the
forgotten slot first: 2 free → (1 next-up, 1 forgotten), 1 free → (1, 0), 0
free → nothing to compose (the board is `Full` before composition ever
runs). Reasoning: with only one slot open, the single most defensible thing
to surface is something plausibly still fresh in mind — a "forgotten,"
deep-backlog item is exactly the kind of thing that needs more context to
act on immediately, which is a worse bet when there is no room to also offer
something easier.

`Blockage` (the reason nothing was offered despite there being room) is
checked as an ordered funnel, most fundamental cause first, so the message
shown is always the *first* thing actually wrong rather than an arbitrary
one among several true statements: backlog empty → no active project → all
blocked → all tangled → all on cooldown. Each check only makes sense once
every check before it has already passed (there is no point reporting "all
on cooldown" if the backlog is empty), so the order is load-bearing, not
incidental.

### The `EveryNWeeks` epoch anchor and `DayOfMonth` clamping

§6 left two edges unspecified. **`EveryNWeeks` anchor**: "every other
Monday" is ambiguous on its own — which Monday is on-cycle? Resolved to a
fixed epoch independent of any particular `from`: the first occurrence of
the given weekday on or after the Unix epoch (1970-01-01, a Thursday), with
valid run-dates at `anchor + k*n` weeks. Anchoring to a fixed point rather
than deriving the cycle from whatever `from` happens to be is what keeps
feeding one call's result back in as the next call's `from` land exactly `n`
weeks later, and what keeps two different `from` values in the same cycle
resolving to the same next occurrence — without a fixed anchor the cadence
would drift, or collapse to "every week," depending on how often the ticker
happened to ask.

**`DayOfMonth` clamping**: a day past a short month's end (the 31st in
April, the 29th–31st in February) clamps to that month's actual last day,
leap-aware. The alternative readings — reject the recurrence outright, or
silently skip the short month entirely — both fail the plain-language intent
of "the 15th" worse than clamping does; clamping is the reading a person
would actually expect.

### Hand-rolled DST replaced by a real IANA tzdb

An earlier version of the timezone handling modeled `Timezone`/`DstRule`
directly — a standard UTC offset plus a hand-written "Nth weekday of the
month" DST rule, evaluated inside `anamnesis-core`. This was a defect, not
an acceptable simplification: real DST rules change by government decree,
sometimes with only weeks of notice (Brazil abolished DST entirely in 2019;
Mexico and Iran dropped most of it in 2022; Jordan and Syria moved
permanently; Chile shifts its dates most years), the whole Southern
Hemisphere's conventions were missing from the hand-curated table, and
critically, reapplying a rule to a *historical* timestamp used whichever
rule happened to be hard-coded *today* rather than whichever rule was
actually in force on that date — silently wrong for anything but a
same-year lookup. No amount of careful hand-rolling fixes that class of bug;
it needs a real tzdb.

Replaced with [`time-tz`](https://docs.rs/time-tz), chosen over `jiff` (also
evaluated) because it fits the `time` crate already used throughout the
workspace with no second date-time representation entering the dependency
graph. Its zone data is vendored from the real IANA tzdata source files and
compiled into the binary at build time as a static lookup table — there is
**no runtime read of `/usr/share/zoneinfo`**, so this works identically in a
container with no system tzdb installed at all. The regression test a
hardcoded table structurally cannot pass: resolving a timestamp from
*before* São Paulo's 2019 DST abolition and asserting it gets the DST-era
offset genuinely in force on that historical date, not the post-2019 rule a
table keyed only on "the current rule" would wrongly project backward onto
it.

### `board_columns`, not `columns`

§3 names the entity `Column`; the stored table is `board_columns` in both
migration trees. Plain `columns` collides badly with SQL's own vocabulary —
`information_schema.columns` in Postgres, plus assorted tooling and shell
completion that assumes `columns` means the metadata table — annoying
enough in practice that the migrations rename the table outright rather
than fight it indefinitely. Only the table name differs from the design
doc; the Rust type stays `Column`.

### Search indexing belongs in the use cases, not the handlers

An early implementation called the `SearchIndex` port directly from
`anamnesis-web`'s HTTP handlers, right after a successful create/edit. That
is a layering mistake this project's own dependency rule (§7, and
`docs/ARCHITECTURE.md`) exists to prevent: any future non-web caller of the
same use cases — the MCP server or CLI `docs/CONTEXT.md` anticipates but
does not build — would silently fail to index anything it wrote, because
indexing lived one layer above where those use cases actually run. Moved
into `anamnesis-app`'s use cases themselves: every use case that touches an
indexable entity calls the index port beside its repository write, so
indexing happens for every caller of that use case, web or otherwise, by
construction rather than by every future caller remembering to add it
itself.

### The `archived` flag on `search_documents`

`SearchIndex::remove_*` flags a search entry as archived rather than
deleting its row. An earlier version deleted the row outright when an
entity was archived, which broke this design's own promise in §2: "vanished
from every view unless explicitly searched" specifically carves out an
*explicit-search* exception, and a hard-deleted row has no path back for
that exception to find. `index_*` — the same call used for create, edit, and
unarchive alike — always resets the flag to not-archived, so unarchiving an
entity re-indexes it through the identical call path as any other edit.
There is still no true row deletion of an area, project, or task anywhere in
this system; `remove_*` is only ever invoked from an archive use case, never
a hard delete, so this is a correction to what "remove" already meant at
every call site, not a new capability.
