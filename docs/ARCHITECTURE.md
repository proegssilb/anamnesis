# Anamnesis — Architecture

> Describes the real domain model (`docs/DOMAIN.md`), live since Phases
> A–F. The placeholder `Board`/`Column`/`Card` kanban this document used to
> describe has been fully retired — see `docs/DOMAIN.md` for why it hit a
> wall (task relationships cross projects; no single aggregate can own the
> graph) and `git log` for the deletion (Phase F1).

Imperative Shell, Functional Core (equivalently: Hexagonal / Ports &
Adapters). The rule that decides every layering question, unchanged since
the placeholder and still exactly right:

> **Anything that can fail because the world is involved lives in the shell.
> Anything that can fail because the rules were broken lives in the core.**

## Dependency direction

```
  anamnesis-web  ─────┐   (binary: axum, minijinja, cookies, config)
                      │
  anamnesis-adapters ─┤   (sqlx, openidconnect, time-tz, system clock, uuid)
                      │
                      ▼
              anamnesis-app          (use cases; DEFINES the port traits)
                      │
                      ▼
              anamnesis-core         (pure domain; zero I/O deps)
```

Arrows point at what a crate is allowed to depend on. Ports are traits
declared in `app`; adapters implement them; `web` wires concrete adapters in
at startup (`crates/anamnesis-web/src/main.rs`).

### The four crates

| Crate | Kind | May depend on | Must never contain |
|---|---|---|---|
| `anamnesis-core` | lib | `serde`, `thiserror`, `time`, `uuid` only | `async`, `sqlx`, `axum`, `tokio`, `reqwest`, filesystem, clock reads, RNG |
| `anamnesis-app` | lib | `core`, `async-trait` | concrete DB/HTTP/OIDC types |
| `anamnesis-adapters` | lib | `app`, `core`, `sqlx`, `openidconnect`, `time-tz` | HTTP routing, templates |
| `anamnesis-web` | bin | all of the above | business rules |

`core` gets `#![forbid(unsafe_code)]`. Its dependency list is exactly four
value-shaped crates — no async runtime, no database, no HTTP, no tzdb — and
`anamnesis-app` adds only `async-trait` on top of that. This is checked by
reading, not by tooling; `cargo tree -p anamnesis-core` staying that short is
what keeps the boundary honest as the crate grows.

## The functional core

Pure, total-where-possible, deterministic. Every state transition is a free
function that takes the current entity plus the intent and returns either a
new entity or a domain error. No mutation in place, no interior state, no
`&mut self` methods that hide effects.

```rust
pub fn move_placement(
    task: &Task,
    to: Placement,
    now: Timestamp,
    settings: &SuggestionSettings,
) -> Result<Task, DomainError>;
```

**Time, identity, and entropy are inputs, never reads.** A function that
needs "now" takes `now: Timestamp`; a function that needs a new id takes the
id as a parameter. This is what makes the core testable without fakes, and
it is non-negotiable — the moment `core` calls a system clock or a random
number generator directly, the architecture is gone.

The suggestion engine is where this rule earns its keep hardest:

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

`suggest` samples from a weighted distribution rather than ranking
top-N (`docs/DOMAIN.md` §5: deterministic ranking starves the middle of the
distribution). Sampling needs randomness, and randomness is exactly the kind
of "the rules were broken" vs "the world was involved" question this
document's opening rule exists to answer: the *policy* of how to weight and
draw is a rule, so it belongs in `core`; the *entropy itself* is a read of
the world, so it stays a parameter the shell supplies. `seed` is derived by
the caller from `(user, local date, board-state fingerprint)`, not fresh
entropy each call, so the same three suggestions persist across a page
refresh — re-rolling on every `F5` would let the user slot-machine for an
easy task.

### Entities, not one aggregate

`docs/DOMAIN.md` §7 states the architectural consequence directly:
**whole-aggregate save is dead.** It was correct for one self-contained
board and wrong for a global graph — task relationships (`Relationship`,
`docs/DOMAIN.md` §3) cross project and area boundaries by design, so no
single aggregate can legally own them, and forcing one to try was the
specific wall the placeholder hit.

In its place: per-entity aggregates, loaded and written independently, each
sized to what genuinely travels together.

| Aggregate | Loaded with | Notes |
|---|---|---|
| `Area` | — | tiny |
| `Project` (`ProjectAggregate`) | field definitions, relationship kinds | config-sized |
| `Task` (`TaskAggregate`) | field values | comments/attachments paged separately |
| `Relationship` | — | standalone edge row, lives outside any project |
| `Tangle` | — | system-derived, reconciled, never edited by hand |
| `Comment`, `Attachment` | — | append-heavy, paged per task |

**`Task` carries `updated_at`-based optimistic concurrency**, the one
checked write in the system: `TaskRepository::update` takes an
`expected_last_touched_at` alongside the new `Task` and fails with
`TaskUpdateError::Conflict` — writing nothing — if the stored row's
`last_touched_at` has moved since the caller read it. This replaces
last-write-wins, which was an accepted, explicitly temporary tradeoff for
the placeholder's single-owner board; with finer-grained per-entity edits
and plausible multi-device use, silently discarding a concurrent edit is no
longer acceptable. No other entity carries this check — `Task` is the one
users are plausibly editing from two places (a phone and a laptop, or two
browser tabs) at once.

### Read models are a separate concern from repositories

`docs/DOMAIN.md` §7 calls this "the single most important structural
addition": the global task board and the suggestion engine's candidate pool
are *queries* across everything above the horizon, not aggregates any
repository owns, so they get their own ports rather than being bolted onto
`TaskRepository`.

- **`BoardQuery`** — `columns_with_items` (the whole board, tasks and placed
  tangles interleaved by position, in column order — a `BoardItem` enum
  rather than two parallel lists because that interleaving is real, not a
  reconstruction the caller should have to do); `count_on_column` and
  `board_state` for WIP-limit checks; `suggestion_candidates` (every
  non-archived task, system-wide, as a `TaskSummary` — eligibility itself
  stays `anamnesis_core::suggest`'s job, not this query's); `blocking_graph`
  (the `BlockingGraph` the engine needs, built fresh each call).
- **`SearchQuery`** — `search` (non-archived hits only) and
  `search_archived` (the explicit-search exception `docs/DOMAIN.md` §2
  names: "vanished from every view unless explicitly searched" promises a
  path back to archived items, and `search_archived` is that path — disjoint
  from `search`'s results, never overlapping).

Both live beside the per-entity repositories in `anamnesis-app::ports`, but
are declared as their own trait families (`crate::ports::query`) rather than
methods tacked onto `TaskRepository`/`ProjectRepository`, because "give me
the board" and "give me this one task" are different shapes of read with
different backing queries — folding them together would leak a query
concern into what should stay a narrow load-one-entity port.

## The imperative shell

Three concentric shells, each thinner than the one inside it:

- **`app` (use cases).** Orchestration only, and it is deliberately boring:
  load through a port, call one pure core function, save through a port, map
  errors. If a use case grows a branch that is really a rule, that branch
  belongs in `core`.
- **`adapters`.** Translation. SQL rows ↔ domain entities, OIDC tokens ↔ an
  authenticated user, the system clock ↔ `Timestamp`, an IANA zone name ↔ a
  real UTC offset.
- **`web`.** Transport. HTTP form bodies ↔ use-case inputs, use-case outputs
  ↔ rendered HTML (or an htmx fragment), domain errors ↔ status codes.

### Port inventory (declared in `anamnesis-app::ports`)

Per-entity repositories (`crate::ports::repository`) — the load-one-entity
side:

```rust
#[async_trait] pub trait AreaRepository: Send + Sync { /* load, list, insert, update */ }
#[async_trait] pub trait ProjectRepository: Send + Sync { /* load, list_by_area, count_active, insert, update, field defs, relationship kinds */ }
#[async_trait] pub trait TaskRepository: Send + Sync {
    /* load, list_children, list_by_project, insert, set_field_value */
    async fn update(&self, task: &Task, expected_last_touched_at: Timestamp)
        -> Result<(), TaskUpdateError>;
}
#[async_trait] pub trait RelationshipRepository: Send + Sync { /* load, list_for_task, list_blocking, insert, delete */ }
#[async_trait] pub trait TangleRepository: Send + Sync { /* list_active, load, insert, update */ }
#[async_trait] pub trait CommentRepository: Send + Sync { /* list_for_task, load, insert, update, delete */ }
#[async_trait] pub trait AttachmentRepository: Send + Sync { /* list_for_task, load, insert, delete */ }
#[async_trait] pub trait SettingsRepository: Send + Sync { /* load, update, record_sweep — the singleton Settings row */ }
```

Read models (`crate::ports::query`) — the query side, described above:

```rust
#[async_trait] pub trait BoardQuery: Send + Sync { /* columns_with_items, count_on_column, board_state, suggestion_candidates, blocking_graph */ }
#[async_trait] pub trait SearchQuery: Send + Sync { /* search, search_archived */ }
```

Infrastructure (`crate::ports::infra`, `crate::ports::common`,
`crate::ports::membership`, `crate::ports::identity`):

```rust
pub trait Clock: Send + Sync { fn now(&self) -> Timestamp; }
pub trait IdGen: Send + Sync { fn next(&self) -> uuid::Uuid; }
#[async_trait] pub trait BlobStore: Send + Sync { /* put, get, delete — attachment file bytes */ }
#[async_trait] pub trait SearchIndex: Send + Sync { /* index_area/project/task, remove_area/project/task */ }
pub trait TimezoneResolver: Send + Sync { /* local_date, local_time, to_utc — not async, a real tzdb lookup is in-memory */ }
#[async_trait] pub trait MembershipQuery: Send + Sync { /* is_system_admin, area_role, project_role, effective_area_role, effective_role, list_system_admins, list_area_members, list_project_members */ }
#[async_trait] pub trait MembershipRepository: Send + Sync { /* grant_system_admin, revoke_system_admin, set_area_role, revoke_area_role, set_project_role, revoke_project_role */ }
#[async_trait] pub trait GroupMembershipQuery: Send + Sync { /* is_system_admin_via_group, area_group_role, project_group_role, effective_area_role, effective_role, list_admin_groups, list_area_groups, list_project_groups, list_known_groups */ }
#[async_trait] pub trait GroupMembershipRepository: Send + Sync { /* replace_user_groups, grant/revoke_admin_group, set/revoke_area_group_role, set/revoke_project_group_role */ }
#[async_trait] pub trait IdentityProvider: Send + Sync { /* begin_login, complete_login */ }
```

`BlobStore` and `SearchIndex` are new ports named directly in
`docs/DOMAIN.md` §7 — attachments and global search did not exist in the
placeholder at all. `TimezoneResolver` is new infrastructure `docs/DOMAIN.md`
requires (§6: "'every other Monday' is meaningless without one") without
naming as a port outright; it exists because `anamnesis-core`'s recurrence
math (`next_run`) works purely in local calendar terms and carries no tzdb of
its own (see below), so something has to convert a UTC instant to "what
local date is it" and back — that something is this port, backed by a real
tzdb in the adapter. `MembershipQuery` is the caller-side role resolver
`anamnesis_core::policy`'s own doc comment calls for: "core has no
membership table to consult, so the caller (which does) resolves 'what role
does this user hold here' before calling in."

`SettingsRepository` (the runtime settings pass) and `MembershipRepository`
(the membership write pass, below) were both flagged as outstanding
follow-ups in earlier revisions of this document and are recorded here now
that both exist. `SettingsRepository` is the one port in this crate with no
id parameter anywhere on it: `Settings` is a genuine singleton (the
active-project limit, the suggestion engine's tunables, the sweep
schedule), so `load`/`update` always mean "the one row" — `record_sweep` is
kept as its own targeted write, isolated from `update`, specifically so the
sweep ticker's write and a concurrent admin edit through `/settings` cannot
race each other in a read-modify-write. `MembershipRepository` is
`MembershipQuery`'s write half, split into its own trait for the same
reason `SearchQuery`/`SearchIndex` are split rather than combined: every
read-only caller (every permission check in `crate::policy`, `view_area`,
`view_project`, ...) only ever needs `MembershipQuery`; only
`crate::use_cases::membership` — the handful of use cases that actually
grant or revoke a role — needs write access at all. Before this port
existed, granting a role was only possible through `SqlStore`'s inherent
seams reached into directly by `anamnesis-web::bootstrap`, which meant the
bootstrap admin was the only user who could ever hold a role anywhere in
the system; see that module's doc comment for what still legitimately
bypasses the use-case layer (bootstrap itself) and why.

The **scheduled sweep ticker** (`anamnesis-web::sweep`, `docs/DOMAIN.md`
§6) is the one piece of shell infrastructure in this system with **no port
of its own** — worth naming here precisely because of that absence. It is
a background `tokio` task, spawned exactly once (from `main.rs`, never from
`routes::build_router`/`AppState` construction/`bootstrap::run`, so no
integration test can ever cause one to spawn) that wakes, asks the pure,
unit-tested `anamnesis_web::sweep::is_due` whether a sweep is due against
`SettingsRepository`-sourced state, and — if so — takes the `archive_sweep`
lease and calls the exact same `anamnesis_app::archive_done_tasks` the
manual "Archive all" button calls, then stamps
`SettingsRepository::record_sweep`. Its interval is not fixed: each pass
picks its own next wake from what that pass resolved, so an ordinary tick
sleeps a day and one that left a due sweep unaccounted for — it failed, or
another instance held the lease — re-checks in minutes.
It needs no port of its own because it is pure orchestration over ports
that already exist (`BoardQuery`, `TaskRepository`, `TangleRepository`,
`SearchIndex`, `Clock`, `SettingsRepository`); the "port" here, such as it
is, is `TimezoneResolver`, already listed above, which is what lets
`is_due` compare a scheduled local date against real elapsed time.

One adapter, `anamnesis_adapters::SqlStore`, implements essentially the
entire repository and query surface against one connection pool (SQLite or
Postgres, chosen at connect time) — see `crates/anamnesis-web/src/main.rs`
for exactly which port each concrete adapter fills at startup:
`SqlStore` for everything above, `FsBlobStore` *or* `S3BlobStore` for
`BlobStore` (chosen from `ANAMNESIS_BLOB_ROOT`'s scheme the same way the
database driver is chosen from its URL),
`TzTimezoneResolver` for `TimezoneResolver`, `SystemClock` for `Clock`,
`UuidIdGen` for `IdGen`, `OidcIdentityProvider` for `IdentityProvider`
(`None` when `ANAMNESIS_DEV_AUTH_BYPASS` is set).

### Area-scoped roles and composition by strongest grant

`docs/DOMAIN.md` §3 names three roles — System Admin, Project Admin, Member
— on a ladder (`Member < ProjectAdmin < SystemAdmin`, `anamnesis_core::policy::Role`).
Roles were originally project-scoped only, which left two problems: purely
area-level actions (viewing an area, managing it) had nowhere to hang except
System Admin, and `CreateProject` was chicken-and-egg — a project that does
not exist yet has no project role to authorize its own creation against. The
fix implemented in `MembershipQuery`: **Areas are a real membership scope
too, and a Project inherits its Area's role** when it carries no explicit
role of its own.

That inheritance alone was not the whole fix. The first version composed
scopes by "most specific wins" — an explicit project-level grant would
override whatever the Area granted, even if the explicit grant was *weaker*.
That silently demoted a System Admin (or an Area Admin) who also happened to
hold a plain Member row on one particular project. The corrected rule is
**composition by strongest grant**, by direct analogy to `chmod`: System
Admin status, the Area grant, and the Project grant are three *independent*
grants, and `effective_role` takes the strongest of the three — adding a
grant must never subtract capability, exactly as adding a permission bit
never removes one already held.

Identity-provider groups (`docs/DOMAIN.md`, "Identity-provider groups as a
fourth grant") add a fourth independent grant on top of those three, and the
shape of that addition is the point worth recording here. `MembershipQuery`'s
`effective_*` methods were **not** widened to know about groups: they remain
the per-user answer, correct on their own and asserted by roughly forty test
sites. The group dimension lives behind its own port pair and composes *over*
them, in three free functions in `anamnesis_app::access` that `.max()` the
two dimensions together. `GroupMembershipQuery` carries its own `effective_*`
defaults mirroring `MembershipQuery`'s, so a group's grants inherit Area to
Project the same way a user's do, and the join of the two is one `.max()` at
the top.

`anamnesis-web::handlers::access` is the **only** production caller of either
side, which keeps exactly one composition point in the system: a handler that
reached for `state.membership` directly would silently deny a user whose
whole access comes through a group, so it doesn't — the module's doc comment
says so, and there is nothing else to consult.

This is a monotonicity property, so it gets a property test rather than a
handful of examples:
`adding_a_further_grant_never_reduces_the_effective_role` in
`crates/anamnesis-app/tests/domain_use_cases.rs` enumerates every
`(project_role, area_role, is_system_admin)` starting state across the four
possible role values (`None`, `Member`, `ProjectAdmin`, `SystemAdmin`) and
every way to strengthen exactly one of those three slots, and asserts the
effective role never goes down. It is exactly the test that caught the
regression above — the individual before/after examples in the same file
still would have passed under "most specific wins" for the *particular*
cases they happened to construct.

## Persistence

`sqlx`, with the backend chosen at runtime from the connection string:

```
sqlite://path/to/file.db?mode=rwc   ->  SQLite
postgres:// | postgresql://          ->  PostgreSQL
```

Anything else is a startup error naming both supported schemes.

Two deliberate constraints, unchanged from the placeholder:

1. **Runtime queries (`sqlx::query`), never the `query!` macros.** The
   macros need a compile-time database connection and bind you to one
   backend; runtime queries cost compile-time checking and buy dual-backend
   support with no `DATABASE_URL` needed to build. The adapter contract test
   (below) is what catches SQL errors instead, so it is mandatory, not
   optional coverage.
2. **Separate migration trees** — `crates/anamnesis-adapters/migrations/sqlite/`
   and `.../postgres/` — because the type vocabularies genuinely differ
   (`TEXT` vs `UUID`, no `TIMESTAMPTZ` in SQLite, SQLite FTS5 vs Postgres
   `tsvector`/GIN for search). The logical schema stays identical; each
   dialect says it its own way.

Schema (both trees, kept in lockstep): `areas`, `projects`,
`field_definitions`, `relationship_kinds`, `board_columns`, `tasks`,
`field_values`, `relationships`, `tangles`, `tangle_tasks`, `comments`,
`attachments`, `system_admins`, `area_members`, `project_members`,
`settings`, `search_documents`.

**`board_columns`, not `columns`.** `docs/DOMAIN.md` §3 names the entity
`Column`; the table is `board_columns`. Plain `columns` collides with SQL's
own reserved/ambiguous vocabulary and with `information_schema.columns` in
Postgres tooling — annoying enough in practice (introspection queries,
ORM-adjacent tooling, even shell completion) that the migrations rename it
outright rather than fight it forever. The Rust type stays `Column`; only
the table name differs from the doc.

**The `archived` flag on `search_documents`.** `SearchIndex::remove_*` flags
an entry as archived rather than deleting the row. An earlier version
deleted it outright, which broke `docs/DOMAIN.md` §2's own contract:
"vanished from every view unless explicitly searched" promises an *explicit*
path back to an archived item, and a hard-deleted row has no path back for
anything to find. `index_*` (the same call used for create, edit, *and*
unarchive) always resets the flag to not-archived, so unarchiving an entity
is exactly the same call as editing it. There is still no true row deletion
of an area, project, or task anywhere in this domain model — `remove_*` is
only ever called from an archive use case, never a hard delete.

**Search indexing happens in the app-layer use cases, not the web
handlers.** An early implementation called `SearchIndex` directly from
`anamnesis-web`'s handlers after a successful create/edit. That is a
layering regression on this document's own dependency rule: any future
non-web caller of the same use cases — the MCP server or a CLI
`docs/CONTEXT.md` anticipates but does not build — would silently fail to
index anything it wrote, because indexing lived in a layer that caller never
passes through. `crates/anamnesis-app/src/use_cases/indexing.rs` now owns
this: every use case that touches an indexable entity calls the index port
itself, beside the repository write, and a failed index write is logged
non-fatally (the entity write already committed by the time indexing runs;
the index is derived, rebuildable data, so a transiently stale search result
is the accepted cost, not a reason to tell the user their edit failed).

### Search

The largest real backend divergence: SQLite FTS5 (`MATCH`, a phrase-quoted
query so user text can never be parsed as FTS5 syntax) vs Postgres
`tsvector`/`tsquery` with a GIN index. Matching semantics differ by design
between the two — this is accepted, not hidden — but the port
(`SearchQuery`/`SearchIndex`) and its contract test are shared, so both
backends are held to the same observable behavior (archived exclusion,
unarchive round-tripping, empty-query handling) even where their ranking
internals diverge.

### Timezones: a real IANA tzdb, not a hand-rolled table

`anamnesis-core`'s recurrence math (`next_run`, `docs/DOMAIN.md` §6) works
purely in local calendar terms — a `Date` in, a `Date` out, no offset
anywhere in sight — because `anamnesis-core` carries no tzdb dependency at
all (its dependency list is exactly `serde`, `thiserror`, `time`, `uuid`).
An earlier version modeled `Timezone`/`DstRule` by hand — a standard offset
plus a hand-written "Nth weekday of the month" DST rule. That was a defect,
not a simplification: real DST rules change by government decree, sometimes
with only weeks of notice (Brazil abolished DST entirely in 2019; Mexico and
Iran dropped most of it in 2022; Jordan and Syria moved permanently; Chile
shifts its dates most years), a hand-curated rule table silently goes stale,
and reapplying a rule to a *historical* timestamp needs whichever rule was
actually in force then — not today's rule projected backward. No amount of
careful hand-rolling fixes that class of bug; it needs a real tzdb.

`anamnesis-adapters::TzTimezoneResolver` (`crates/anamnesis-adapters/src/timezone.rs`)
is backed by [`time-tz`](https://docs.rs/time-tz), chosen over `jiff` (also
evaluated) because it fits the `time` crate already used throughout the
workspace with no second date-time representation in the dependency graph.
Its `db` feature vendors real IANA tzdata *source files* inside the crate
itself and bakes the parsed result into the binary as a static map at
compile time — there is no runtime read of `/usr/share/zoneinfo` anywhere in
this path, so a container with no system tzdb installed behaves identically
to one that has the latest copy, because neither is ever consulted.
"Freshness" is pinned to whichever `time-tz` version is vendored, not to the
host OS.

The regression test a hand-curated table structurally cannot pass:
`sao_paulo_historical_date_uses_the_rule_in_force_then_not_todays`
(`crates/anamnesis-adapters/src/timezone.rs`) resolves a timestamp from
*before* Brazil abolished DST in 2019 and asserts it gets the DST-era offset
that was actually in force on that date, not the post-2019 permanent
standard-time rule a table keyed only on "current rule" would wrongly apply
retroactively.

## Authentication

Unchanged from the placeholder. OAuth2 Authorization Code + PKCE against
**any** OIDC provider, discovered from its issuer URL
(`/.well-known/openid-configuration`). Authentik is the reference deployment
and gets zero special-casing in code — a provider-specific branch would be a
bug.

Flow: `GET /login` → redirect to provider → `GET /auth/callback` → exchange
code → validate ID token (signature, issuer, audience, nonce) → establish
session. Identity is the `sub` claim, stored as `UserId`. Anamnesis never
sees a password and stores no credential.

The session is a signed, `HttpOnly`, `SameSite=Lax`, `Secure`-when-HTTPS
cookie holding the user id, display name, and a CSRF token. Every mutating
form (and htmx request) embeds that CSRF token and the handler rejects
mismatches.

`ANAMNESIS_DEV_AUTH_BYPASS=1` short-circuits to a fixed local user so
development and HTTP integration tests do not need a live IdP. It logs a
loud warning on every startup and must never be set in a real deployment.

### Bootstrap

A fresh database has no System Admin (nobody can grant one — `create_area`
and friends are System-Admin-gated) and no board columns to place a task on.
`crates/anamnesis-web/src/bootstrap.rs` runs once at startup, idempotently,
before the router accepts requests: grants `ANAMNESIS_BOOTSTRAP_ADMIN`
System Admin if that subject does not already hold it, and seeds the three
default board columns (To-Do, WIP-limited; Doing; Done) if none exist yet.
Both halves are safe to run on every boot — the grant is a no-op once the
named subject already holds System Admin, and columns seed only when
`BoardQuery::columns_with_items` reports zero.

## UI: htmx, PWA-ready

The placeholder shipped zero JavaScript by explicit constraint. The real
model relaxed that (owner's call, `docs/DOMAIN.md` §8): **htmx +
`hx-trigger`**, plus a drag library (Sortable) for board cards specifically.

**Why both, and why that split.** htmx and a drag library do different jobs
and neither covers the other: htmx has no drag support at all (it fires
requests on events), and a drag library supplies pointer/touch mechanics and
live drag feedback but no transport. The division is **Sortable drags, htmx
persists** — `onEnd` triggers an htmx request that swaps the returned
fragment. Native HTML5 drag-and-drop (no library at all) was rejected on one
ground: it is unreliable on touch devices, and a mobile PWA is a stated
product goal.

**The surface that drags is deliberately small.** Only board cards need
dragging. Custom-field ordering is rare and admin-shaped, served by plain
up/down form-POST buttons — no library, touch-friendly by default.

Carried over from the placeholder, all still true:

- **Form-POST fallbacks stay live everywhere** htmx also handles — the app
  keeps working without JS, and every route keeps its PWA-ready,
  resource-oriented shape (no RPC-shaped catch-alls) for a future
  online-only PWA client that content-negotiates JSON instead of HTML.
- **Mobile-first CSS**, `manifest.webmanifest` served now, semantic HTML
  with stable ids/classes for real client hooks.
- **No offline story.** "Online-only" is a licence to skip caching, sync,
  and conflict resolution entirely.
- **No real-time collaboration, no server push — a refresh is the update
  mechanism**, by decision, not omission.

Fragment templates are addressable per-piece (one card, one column, the
search results list, the suggestion prompt) so htmx can swap precisely
rather than re-rendering the whole page on every interaction.

## Testing strategy

TDD throughout — the test is written first, watched fail for the right
reason, then made to pass. Four layers, each testing something the others
cannot:

| Layer | Tool | What it proves |
|---|---|---|
| `core` | `#[test]` + `rstest` (+ a property test for role monotonicity) | The rules are right. Fast, pure, no fakes needed. |
| `app` | **`cucumber` (Gherkin)**, feature files under `crates/anamnesis-app/features/` | The behaviours the owner asked for happen, described in their language, against in-memory fakes. |
| `adapters` | `sqlx` against a temp SQLite file, plus Postgres gated on `ANAMNESIS_TEST_PG_URL`; **one shared contract test function run against both backends** | The SQL is actually valid and behaviourally identical on both backends — they cannot drift apart unnoticed. |
| `web` | `tower::ServiceExt::oneshot` | Routing, forms, htmx fragments, redirects, auth gating, CSRF, status codes — no socket. |

BDD lives at the `app` layer on purpose: high enough to read as behaviour,
low enough to run in milliseconds without a browser or a server socket.
Feature files are the readable spec — prose in `docs/` can drift; `.feature`
files cannot, because they fail.

**The adapter contract is one function, not two.** `sql_store_contract.rs`
defines the behavioural assertions once and calls them from both a SQLite
test (always runs) and a Postgres test (`#[ignore]`d unless
`ANAMNESIS_TEST_PG_URL` is set — see the Postgres job in
`.github/workflows/ci.yml`), so a bug in one backend's SQL that the other
gets right cannot silently pass — the same assertions run against both.
