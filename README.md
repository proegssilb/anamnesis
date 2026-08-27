# anamnesis
What's below the horizon isn't gone - it's just not up yet.

_ἀνάμνησις_ — recollection; the recovery of knowledge you already had.

## What this is

Anamnesis is a personal task tracker built on one premise: **tasks that
aren't currently relevant shouldn't be deleted and shouldn't nag.** Dump a
task in, trust the system to resurface it at the right moment, and stop
carrying it in your head. It's designed for someone whose attention is
interrupted — losing your scroll position, losing your place in a flow,
coming back after three weeks — all normal, none of it treated as an error
state.

See `docs/DOMAIN.md` for the full design and its reasoning, and
`docs/CONTEXT.md` for the product framing and its provenance.

### The horizon

Every task sits in exactly one of three places:

- **Below the horizon** — the backlog. Exists, keeps its relationships,
  costs nothing against your work-in-progress. Where most tasks live most
  of the time.
- **Above, on the task board** — in a column (To-Do, Doing, Done by
  default). The column *is* the status.
- **Archived** — vanished from every view unless you explicitly search for
  it.

There is deliberately no size/estimate field anywhere. Weight is *observed*
from data already captured for other reasons — how big a checklist a task
has, how stale it's gotten, and especially **bounce count**: how many times
it's been raised above the horizon and dropped back down unfinished. A
task that keeps bouncing gets a different, gentler prompt than one just
sitting there, because bouncing is a much better signal of real resistance
than any estimate you'd type in at capture time.

### Tangles

Task dependencies (`blocks` / `blocked by`, plus free-form labels like
"relates to") are edges that can cross project and area boundaries and,
because real work is like this, can form cycles — mutually blocking
clusters that no single "the blocker" narrative resolves. Anamnesis detects
these as **tangles** (one per strongly-connected component of the blocking
graph, via Tarjan's algorithm) and surfaces the tangle itself, as its own
board card, in place of the individually-unactionable tasks caught in it.
Untangling is real work, so a tangle can be raised onto the board and
occupies a WIP slot exactly like a task — and once its member tasks no
longer form a cycle, it resolves automatically into the Done column, so you
see the knot actually close.

### The suggestion engine

The soul of the product, and a pure, deterministic function of `(now, a
random seed, board state, candidate tasks, the blocking graph, settings)` —
no hidden clock reads, no hidden randomness, which is also exactly what
makes it heavily tested (see `docs/DOMAIN.md` §5 and the `cucumber` features
under `crates/anamnesis-app/features/`).

It fills open work-in-progress slots with up to three offers — two sampled
from a recency-weighted distribution ("next up"), one from a
staleness-weighted distribution over the older half of the backlog
("forgotten") — using weighted sampling, not a top-N ranking, so that a task
which has simply never been touched still has a real chance of surfacing
instead of waiting behind whatever's already most-recent forever. And it
follows one rule above all: **when the board is already full, it says
nothing at all.** No banner, no nudge. A full board means you're already
carrying what you agreed to carry; the app has nothing useful to add. It
only speaks up when there's room and nothing to fill it with — and then it
explains, concretely, why (nothing's active, everything's blocked,
everything's tangled, everything's on cooldown, or the backlog is simply
empty).

## Quickstart (SQLite + dev auth bypass)

This is the fast path for running Anamnesis locally with no external
identity provider — every request is authenticated as a fixed local user.
**Never do this in a real deployment**; see the OIDC section below for real
auth.

The exact sequence below (fresh SQLite file, dev bypass, `cargo run`, then
driving the app through a full area → project → task → tangle → suggestion
→ search → archive-all loop over HTTP) was run against this README while
writing it, from an empty working directory on an unused port, to confirm
it works as written. The server was killed afterward.

### Prerequisites

- Rust (stable toolchain — edition 2024, so a reasonably recent `rustc`;
  install via [rustup](https://rustup.rs) if you don't have one)
- `openssl` (or any way to generate a 64+ byte random string) for the
  session secret
- No database server needed — SQLite is a file, created on first run

### Steps

From the repository root:

```sh
# 1. A session secret of at least 64 bytes. Generate one:
export ANAMNESIS_SESSION_SECRET="$(openssl rand -hex 64)"

# 2. The rest of the required configuration:
export ANAMNESIS_DATABASE_URL="sqlite://anamnesis.db?mode=rwc"
export ANAMNESIS_BASE_URL="http://localhost:8080"
export ANAMNESIS_DEV_AUTH_BYPASS=1
export ANAMNESIS_TIMEZONE="America/New_York"          # any IANA zone name
export ANAMNESIS_BOOTSTRAP_ADMIN="dev-user"            # matches the bypass user

# 3. Run it.
cargo run --bin anamnesis-web
```

You'll see a loud warning that dev auth bypass is on (expected), then two
bootstrap lines and a listening line:

```
WARN anamnesis_web: ANAMNESIS_DEV_AUTH_BYPASS is enabled: ...
INFO anamnesis_web::bootstrap: bootstrap: granted System Admin (ANAMNESIS_BOOTSTRAP_ADMIN) user=dev-user
INFO anamnesis_web::bootstrap: bootstrap: seeded default board columns (To-Do, Doing, Done)
INFO anamnesis_web: anamnesis listening addr=0.0.0.0:8080
```

Open **http://localhost:8080/areas** in a browser — you're already "logged
in" as `dev-user`, who was just granted System Admin, so you can create an
area, a project inside it (set it to **Active** — the suggestion engine and
task creation both need an active project), and tasks inside that. From
**http://localhost:8080/board** you can raise a task or accept a
suggestion, drag cards between columns, add a `blocks` relationship both
ways between two tasks to watch a tangle appear (and, once you remove one
side, watch it resolve into Done on its own), and search from
**http://localhost:8080/search?q=...**. `GET /healthz` returns `200 ok`
for a quick liveness check.

`.env.example` in the repo root documents every variable this binary reads
— including the ones this quickstart skips (`ANAMNESIS_BIND_ADDR`, the
`ANAMNESIS_OIDC_*` family, `ANAMNESIS_BLOB_ROOT`). Anamnesis reads the
process environment directly — it does not load `.env` itself — so use
`direnv`, `dotenvx run`, or export the variables as shown above.

## Real authentication: OpenID Connect

Anamnesis never stores a password. Login is OAuth2 Authorization Code +
PKCE against **any OIDC-compliant provider**, discovered from its issuer URL
(`/.well-known/openid-configuration`). There is no provider-specific code in
Anamnesis — if you ever find a branch that special-cases one identity
provider, that's a bug.

### Worked example: Authentik

[Authentik](https://goauthentik.io/) is *an* example provider, used while
building this system because it was the reference deployment — not a
requirement. Any OIDC-compliant provider (Keycloak, Zitadel, Okta, Auth0,
Google Workspace, ...) works identically: register a confidential OAuth2/OIDC
client, point its redirect URI at `${ANAMNESIS_BASE_URL}/auth/callback`, and
give Anamnesis the issuer URL and credentials.

1. In Authentik, create a **Provider**:
   - Type: **OAuth2/OpenID Provider**
   - Client type: **Confidential**
   - Redirect URIs / Origins: `http://localhost:8080/auth/callback` (or your
     real `ANAMNESIS_BASE_URL` + `/auth/callback` in production — Anamnesis
     builds this URL itself from `ANAMNESIS_BASE_URL`, so it must match
     exactly)
   - Scopes: `openid`, `profile`, `email` (Anamnesis's default
     `ANAMNESIS_OIDC_SCOPES`)
   - Signing key: any available RSA/EC key — Anamnesis validates the ID
     token signature via keys published at the provider's JWKS endpoint,
     discovered automatically
2. Create an **Application** in Authentik and attach it to that provider.
   Note the generated **Client ID** and **Client Secret**.
3. Configure Anamnesis with the provider's issuer URL and those credentials:

   ```sh
   unset ANAMNESIS_DEV_AUTH_BYPASS
   export ANAMNESIS_OIDC_ISSUER_URL="https://your-authentik-host/application/o/<application-slug>/"
   export ANAMNESIS_OIDC_CLIENT_ID="<client id from step 2>"
   export ANAMNESIS_OIDC_CLIENT_SECRET="<client secret from step 2>"
   export ANAMNESIS_OIDC_SCOPES="openid profile email"
   # ANAMNESIS_BOOTSTRAP_ADMIN must now be the real sub claim your provider
   # will issue for you, not "dev-user".
   export ANAMNESIS_BOOTSTRAP_ADMIN="<your provider's sub claim for you>"
   ```

4. `cargo run --bin anamnesis-web`, then visit `/login` — you'll be
   redirected to your provider, and back to `/auth/callback` on success.

## Running the tests

Each layer of `docs/ARCHITECTURE.md`'s hexagon has its own test tool,
testing something the others can't.

```sh
# Everything except the Postgres side of the adapter contract test (which
# is #[ignore]d by default so `cargo test` stays green with no database
# running):
cargo test --workspace

# anamnesis-core only — pure domain logic: placement, containment,
# relationships, tangle detection, the suggestion engine, recurrence.
# #[test] + rstest, no fakes needed:
cargo test -p anamnesis-core

# anamnesis-app only — includes the cucumber BDD suite (feature files in
# crates/anamnesis-app/features/: access_control, placement, suggestions,
# tangles), driving real use cases against in-memory fakes. Wired as a
# `[[test]] harness = false` target, so it runs as part of the normal
# `cargo test` for this crate too:
cargo test -p anamnesis-app

# anamnesis-adapters — the SqlStore contract (one shared set of assertions,
# run against both backends so they can't silently drift apart):
cargo test -p anamnesis-adapters                                  # SQLite side only
ANAMNESIS_TEST_PG_URL="postgres://user:pass@localhost/db" \
  cargo test -p anamnesis-adapters --test sql_store_contract -- --ignored   # Postgres side

# anamnesis-web — HTTP integration tests via tower::ServiceExt::oneshot
# (routing, htmx fragments, redirects, auth gating, CSRF, status codes),
# no socket:
cargo test -p anamnesis-web
```

Before committing, also run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

CI (`.github/workflows/ci.yml`) runs all of the above on every push and pull
request, including the Postgres side of the adapter contract test against a
real `postgres:16` service container.

## Architecture

Imperative Shell / Functional Core across four crates (`anamnesis-core` →
`anamnesis-app` → `anamnesis-adapters` → `anamnesis-web`), with dependencies
pointing one way only. The full write-up — the dependency rule, the port
inventory, per-entity persistence and optimistic concurrency, dual
SQLite/Postgres, OIDC, and the four-layer test strategy — lives in
`docs/ARCHITECTURE.md`. Read that before changing anything structural; this
README stays a pointer to it rather than a second copy.

## Status: what doesn't work yet

Stated plainly rather than discovered the hard way:

- **No MCP server.** Anticipated (`docs/CONTEXT.md`), not built. There is
  no CLI either — the web UI is the only client today.
- **Checklists have limited UI.** The model fully supports them (a
  checklist item is just a task with `parent_task_id` set, independently
  placeable above or below the horizon), but the only way to attach one
  task to another as a checklist item is pasting the parent's raw task id
  into a plain text field on the task page — there is no "add checklist
  item" affordance from the parent's own view yet.
- **No recurring tasks.** `Recurrence` (`EveryNWeeks` / `DayOfMonth` /
  `Never`) exists purely to drive the archive sweep's schedule; nothing lets
  a *task itself* recur. `docs/DOMAIN.md` §6 names this as a deliberate,
  reusable-later type, not an oversight.
- **No scheduled archive sweep actually runs.** `next_run` and
  `sweep_done` are pure, fully tested functions, and the manual **Archive
  all** button on the board calls `sweep_done` directly and works — but no
  background ticker calls `next_run` on any timer. Until one is wired into
  `anamnesis-web`'s startup, archiving completed work is a manual action
  only, regardless of what a project's configured recurrence would imply.
- **No web UI to grant roles to anyone but the bootstrap admin.**
  `ANAMNESIS_BOOTSTRAP_ADMIN` gets System Admin on first boot (idempotently,
  every boot); after that, adding another user as an Area/Project
  Admin or Member has no HTTP route at all — it exists only as inherent,
  untested-from-the-UI seams on `SqlStore` (`set_area_role`,
  `set_project_role`) that nothing in `anamnesis-web` currently calls.
  Practically, today, this is a single-admin system unless someone edits
  the database directly.
- **`Settings` (active project limit, suggestion cooldown, high-bounce
  threshold, sweep recurrence) aren't editable at runtime.** They're
  compiled-in constants in `crates/anamnesis-web/src/settings.rs`, not a
  real read from the `settings` table the schema already has — there is no
  `SettingsRepository` port yet.
- **A resolved tangle sitting in the Done column is never cleared by
  "Archive all."** `sweep_done` only knows about `Task`s (a `Tangle` has no
  `archived_at` to sweep), so a resolved tangle card accumulates in Done
  permanently once you've untangled it — there's no route to delete or
  archive a `Tangle` row at all.
- **No real-time updates.** No websockets, no server push — a page refresh
  is the update mechanism. This one's by design, not a gap: see
  `docs/DOMAIN.md` §8.
- **`docs/CONTEXT.md` is still a stub.** The project owner's accumulated
  context lives in Claude Chat memories that have not been transcribed into
  this repository yet; the file says so explicitly and will be superseded
  once they land.
