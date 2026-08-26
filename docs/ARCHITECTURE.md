# Anamnesis — Architecture

Imperative Shell, Functional Core (equivalently: Hexagonal / Ports & Adapters).
The rule that decides every layering question:

> **Anything that can fail because the world is involved lives in the shell.
> Anything that can fail because the rules were broken lives in the core.**

## Dependency direction

```
  anamnesis-web  ─────┐   (binary: axum, minijinja, cookies, config)
                      │
  anamnesis-adapters ─┤   (sqlx, openidconnect, system clock, uuid)
                      │
                      ▼
              anamnesis-app          (use cases; DEFINES the port traits)
                      │
                      ▼
              anamnesis-core         (pure domain; zero I/O deps)
```

Arrows point at what a crate is allowed to depend on. `core` depends on
nothing but `serde`/`uuid`/`time`-style value libraries. `adapters` and `web`
depend on `app`; `app` never depends on them. Ports are traits declared in
`app`; adapters implement them; `web` wires concrete adapters in at startup.

### The four crates

| Crate | Kind | May depend on | Must never contain |
|---|---|---|---|
| `anamnesis-core` | lib | value libs only | `async`, `sqlx`, `axum`, `tokio`, `reqwest`, filesystem, clock reads, RNG |
| `anamnesis-app` | lib | `core`, `async-trait` | concrete DB/HTTP/OIDC types |
| `anamnesis-adapters` | lib | `app`, `core`, `sqlx`, `openidconnect` | HTTP routing, templates |
| `anamnesis-web` | bin | all of the above | business rules |

`core` gets `#![forbid(unsafe_code)]` and a CI check that its dependency tree
contains no async runtime. That check is what keeps the boundary honest once
the "massive reorganization" starts.

## The functional core

Pure, total-where-possible, deterministic. Every state transition is a free
function that takes the current aggregate plus the intent and returns either a
new aggregate or a domain error. No mutation in place, no interior state, no
`&mut self` methods that hide effects.

```rust
pub fn move_card(
    board: &Board,
    card: CardId,
    to_column: ColumnId,
    to_index: usize,
) -> Result<Board, DomainError>;
```

Time and identity are **inputs, never reads**. A function that needs "now"
takes `now: Timestamp` as a parameter; a function that needs a new id takes the
id as a parameter. This is what makes the core testable without fakes, and it
is non-negotiable — the moment `core` calls `Utc::now()` or `Uuid::new_v4()`
the architecture is gone.

### Aggregate

`Board` is the single consistency boundary. A board owns its columns; a column
owns its ordered cards. Card ordering is the `Vec` order — canonical, in
memory, in the core. Persisted `position` integers are an adapter concern and
are derived from the `Vec` on write, never trusted on read beyond sorting.

```rust
Board  { id, owner: UserId, title: Title, columns: Vec<Column> }
Column { id, title: Title, wip_limit: Option<u16>, cards: Vec<Card> }
Card   { id, title: Title, body: String, created_at: Timestamp }
```

`Title` is a newtype with a validating constructor (trimmed, non-empty, max
200 chars). Parse, don't validate: once you hold a `Title`, it is valid, and
no downstream layer re-checks it.

## The imperative shell

Three concentric shells, each thinner than the one inside it:

- **`app` (use cases).** Orchestration only, and it is deliberately boring:
  load the aggregate through a port, call one pure core function, save the
  result through a port, map errors. If a use case grows a branch that is
  really a rule, that branch belongs in `core`.
- **`adapters`.** Translation. SQL rows ↔ domain aggregates, OIDC tokens ↔ an
  authenticated user, the system clock ↔ `Timestamp`.
- **`web`.** Transport. HTTP form bodies ↔ use-case inputs, use-case outputs ↔
  rendered HTML, domain errors ↔ status codes.

### Ports (declared in `app`)

```rust
#[async_trait] pub trait BoardRepository: Send + Sync {
    async fn load(&self, id: BoardId) -> Result<Option<Board>, RepoError>;
    async fn save(&self, board: &Board) -> Result<(), RepoError>;
    async fn list_for_owner(&self, owner: &UserId) -> Result<Vec<BoardSummary>, RepoError>;
    async fn delete(&self, id: BoardId) -> Result<(), RepoError>;
}
pub trait Clock: Send + Sync { fn now(&self) -> Timestamp; }
pub trait IdGen: Send + Sync { fn next(&self) -> Uuid; }
#[async_trait] pub trait IdentityProvider: Send + Sync { /* see below */ }
```

`save` writes the whole aggregate in one transaction (delete-and-reinsert the
board's columns and cards). This is last-write-wins and will lose a concurrent
edit. That is an accepted, documented tradeoff for the placeholder: it keeps
the core free of persistence-shaped compromises, which is the property we
actually care about right now. Revisit when the model is reorganized.

## Persistence

`sqlx`, with the backend chosen at runtime from the connection string:

```
sqlite://anamnesis.db?mode=rwc   ->  SQLite
postgres:// | postgresql://      ->  PostgreSQL
```

Anything else is a startup error naming both supported schemes.

Two deliberate constraints:

1. **Runtime queries (`sqlx::query`), not the `query!` macros.** The macros
   need a compile-time database and bind you to one backend. Runtime queries
   cost compile-time checking and buy dual-backend support with no `DATABASE_URL`
   needed to build. For the placeholder that trade is correct; the adapter's
   integration tests are what catch SQL errors instead, so they are mandatory.
2. **Separate migration trees** — `migrations/sqlite/` and `migrations/postgres/` —
   because the type vocabularies genuinely differ (`TEXT` vs `UUID`,
   `INTEGER` vs `INT`, no `TIMESTAMPTZ` in SQLite). Keep the logical schema
   identical; let each dialect say it its own way.

Schema: `boards`, `columns`, `cards`. Ordering is a `position INTEGER NOT NULL`
per parent, written from the `Vec` index, read back with `ORDER BY position`.
`UUID`s are stored as `TEXT` in SQLite and native `uuid` in Postgres; the
adapter absorbs the difference.

## Authentication

OAuth2 Authorization Code + PKCE against **any** OIDC provider, discovered from
its issuer URL (`/.well-known/openid-configuration`). Authentik is the reference
deployment and gets exactly zero special-casing in code — if a provider-specific
branch ever appears, it is a bug.

Flow: `GET /login` → redirect to provider → `GET /auth/callback` → exchange code
→ validate ID token (signature, issuer, audience, nonce) → establish session.
Identity is the `sub` claim, stored as `UserId`. Anamnesis never sees a password
and stores no credential.

The session is a signed, `HttpOnly`, `SameSite=Lax`, `Secure`-when-HTTPS cookie
holding the user id, display name, and a CSRF token. Every mutating form embeds
that CSRF token in a hidden field and the handler rejects mismatches — cheap, and
it is a real security boundary rather than placeholder scaffolding.

`ANAMNESIS_DEV_AUTH_BYPASS=1` short-circuits to a fixed local user so that
development and HTTP integration tests do not need a live IdP. It logs a loud
warning on every startup and must never be set in a real deployment.

## UI: no JavaScript now, PWA-ready later

Server-rendered HTML via MiniJinja, one hand-written stylesheet, zero JS.
Interaction is plain forms: each card carries submit buttons for its moves, each
column a form to add a card. Every POST answers `303 See Other` back to the
board.

**Losing your place is explicitly acceptable here** — it is a stated product
constraint, not a defect to engineer around. The one cheap concession is that
redirects carry a `#card-{id}` fragment so the browser lands near what you just
touched. Nothing more elaborate than that until the reorganization.

Being *ready* for an online-only PWA, without shipping any JS today, means:

- **Mobile-first CSS.** Single-column stack by default; columns become a
  horizontally scrolling flex row at wider breakpoints. Touch targets ≥44px.
  `<meta name="viewport">`, `theme-color`, and safe-area insets from day one.
- **A real `manifest.webmanifest`** served now (name, icons, `display:
  standalone`, `start_url`), so installability is a matter of adding a service
  worker later rather than retrofitting the shell.
- **Routes shaped so a PWA can reuse them.** Every mutation is one resource-
  oriented endpoint doing one thing, so the future app can call the same URL and
  content-negotiate JSON instead of HTML. No RPC-shaped catch-all handlers.
- **Semantic HTML with stable ids/classes** (`#card-{uuid}`, `.column`,
  `.card`), so a client renderer has real hooks.
- **No offline story.** "Online-only" is a licence to skip caching, sync, and
  conflict resolution entirely. Do not build them.

## Testing strategy

TDD throughout — the test is written first, watched fail, then made to pass.
Three layers, each testing something the others cannot:

| Layer | Tool | What it proves |
|---|---|---|
| `core` | `#[test]` + `rstest` | The rules are right. Fast, pure, no fakes needed. |
| `app` | **`cucumber` (Gherkin)** | The behaviours the owner asked for happen, described in their language. Drives real use cases against in-memory fakes. |
| `adapters` | `sqlx` against a temp SQLite file; Postgres gated on env | The SQL is actually valid on both backends. |
| `web` | `tower::ServiceExt::oneshot` | Routing, forms, redirects, auth gating, status codes. |

BDD lives at the `app` layer on purpose: high enough to read as behaviour,
low enough to run in milliseconds without a browser or a server socket. Feature
files are the readable spec — `docs/` prose can drift, `.feature` files cannot,
because they fail.
