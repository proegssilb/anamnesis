# Anamnesis — Placeholder Scaffold Plan

Execution plan for the first working slice. Read `ARCHITECTURE.md` first; it is
binding, this file is the running order. Phases are sequential — each assumes
the previous one is committed and green.

## Ground rules for every phase

1. **TDD, strictly.** Write the failing test, run it, watch it fail for the
   right reason, then implement. A commit that adds behaviour without a test
   that would have caught its absence is not done.
2. **`cargo fmt` + `cargo clippy --all-targets -- -D warnings` clean** before
   every commit.
3. **`cargo add`, never hand-written versions.** Resolve current versions from
   the registry; do not guess a version number from memory.
4. Edition 2024. `#![forbid(unsafe_code)]` in every crate.
5. Commit per phase with a message naming the phase. Do not open a PR.
6. **Do not widen scope.** No MCP. No offline/PWA JavaScript. No features
   beyond the phase you are on. This whole thing gets reorganized later; the
   value is in the boundaries, not in the kanban.

---

## Phase 1 — Workspace + functional core

Cargo workspace at the repo root; member crates under `crates/`.

`anamnesis-core` — pure, no async, no I/O. Dependency list should be roughly
`uuid`, `time` (or `jiff`), `serde`, `thiserror`. Nothing else.

Deliver:
- `Title` newtype: trimmed, non-empty, ≤200 chars, validating constructor.
  Parse-don't-validate — no downstream layer re-checks a `Title`.
- Id newtypes `BoardId` / `ColumnId` / `CardId` (wrapping `Uuid`) and
  `UserId(String)`. `Timestamp` newtype.
- `Board` / `Column` / `Card` per ARCHITECTURE.md.
- `DomainError`: `ColumnNotFound`, `CardNotFound`, `DuplicateId`, `InvalidTitle`,
  `WipLimitExceeded`, `ColumnNotEmpty`.
- Pure transitions, each `fn(&Board, ...) -> Result<Board, DomainError>`:
  `create_board`, `add_column`, `rename_column`, `remove_column`, `add_card`,
  `edit_card`, `remove_card`, `move_card`.
- `fn can_view(board: &Board, user: &UserId) -> bool` — ownership policy is a
  domain rule, not an app-layer `if`.

Tests that must exist (these are the ones that actually bite):
- Moving a card **downward within its own column** lands at the requested index
  after removal, not one short of it. Classic off-by-one; write it first.
- Moving to `to_index` beyond the column length clamps to the end rather than
  erroring.
- A same-column move does **not** trip a full column's WIP limit (the card is
  already counted); a cross-column move into a full column does.
- `remove_column` on a non-empty column is `ColumnNotEmpty`.
- Titles: whitespace-only rejected, surrounding whitespace trimmed, 201 chars
  rejected, 200 accepted.
- Every transition leaves the board's ids unique.

No clock and no RNG anywhere in this crate — `now` and new ids arrive as
parameters. Add a CI-checkable assertion of that boundary if it is cheap.

## Phase 2 — Application layer + BDD

`anamnesis-app`. Declares the port traits (see ARCHITECTURE.md), implements use
cases, owns `AppError` (`NotFound`, `Forbidden`, `Domain(DomainError)`,
`Repo(RepoError)`).

Use cases, one struct or function each, each doing load → pure core call →
save: `CreateBoard`, `ListBoards`, `ViewBoard`, `AddColumn`, `AddCard`,
`MoveCard`, `EditCard`, `DeleteCard`, `DeleteBoard`. Every one that touches an
existing board checks `can_view` first and returns `Forbidden` on failure —
including the read paths.

Test doubles in `tests/` (not in `src/`, they are not production code): an
in-memory `BoardRepository`, a `FixedClock`, a `SequentialIdGen` handing out
deterministic ids, and a `StubIdentityProvider`.

**BDD with the `cucumber` crate.** Feature files in `crates/app/features/`,
steps driving the real use cases against the fakes:
- `board_management.feature` — create a board, add columns, add cards, rename,
  delete.
- `card_movement.feature` — move within a column, move across columns, WIP
  limits, index clamping. Write these as scenarios the owner would recognise.
- `authorization.feature` — a second user can neither read nor mutate another
  user's board.

Wire cucumber as a `[[test]]` target with `harness = false` so `cargo test`
runs it.

## Phase 3 — Adapters

`anamnesis-adapters`. Implements the Phase 2 ports.

- `SystemClock`, `UuidIdGen` — trivial, but they are what keep the core pure.
- **`SqlBoardRepository`.** Backend enum over `SqlitePool` / `PgPool`, selected
  by connection-string scheme; unknown scheme is a startup error naming both
  supported forms. Runtime `sqlx::query` only — no `query!` macros. `save`
  writes the whole aggregate in one transaction. `position` written from `Vec`
  index, read with `ORDER BY position`.
- Migrations in `migrations/sqlite/` and `migrations/postgres/`, same logical
  schema, dialect-appropriate types. Run on startup.
- **`OidcIdentityProvider`** via the `openidconnect` crate: discovery from the
  issuer URL, Authorization Code + PKCE, full ID-token validation (signature,
  issuer, audience, nonce). `sub` becomes `UserId`. Zero Authentik-specific
  code — it is just an OIDC provider.

Tests: the full repository contract exercised against a **temporary SQLite
file** (not `:memory:` — a pool opens multiple connections and each would get
its own empty database). Round-trip a board with several columns and cards and
assert ordering survives. The same contract runs against Postgres when
`ANAMNESIS_TEST_PG_URL` is set and is `#[ignore]`d when it is not — write it as
one shared contract function called by both, so the backends cannot drift.

## Phase 4 — Web shell

`anamnesis-web`, the binary. Axum + MiniJinja. No JavaScript.

Config from environment, validated once at startup into a typed struct;
missing required values fail loudly with the variable name:

| Variable | Notes |
|---|---|
| `ANAMNESIS_DATABASE_URL` | required; `sqlite://` or `postgres://` |
| `ANAMNESIS_BIND_ADDR` | default `0.0.0.0:8080` |
| `ANAMNESIS_BASE_URL` | required; builds the OAuth redirect URI |
| `ANAMNESIS_OIDC_ISSUER_URL` | required unless dev bypass |
| `ANAMNESIS_OIDC_CLIENT_ID` / `_CLIENT_SECRET` | required unless dev bypass |
| `ANAMNESIS_OIDC_SCOPES` | default `openid profile email` |
| `ANAMNESIS_SESSION_SECRET` | required, ≥64 bytes, rejected if shorter |
| `ANAMNESIS_DEV_AUTH_BYPASS` | dev/test only; loud warning every startup |

Routes — resource-oriented, one job each, so a future PWA can call the same
URLs and content-negotiate JSON:

```
GET  /healthz
GET  /                                    -> 303 /boards
GET  /login | GET /auth/callback | POST /logout
GET  /boards            POST /boards
GET  /boards/{id}       POST /boards/{id}/delete
POST /boards/{id}/columns
POST /boards/{id}/columns/{cid}/cards
POST /boards/{id}/cards/{card_id}/move        (form: to_column, to_index)
POST /boards/{id}/cards/{card_id}/delete
GET  /static/app.css    GET /manifest.webmanifest
```

Every mutating POST answers `303 See Other` to the board with a `#card-{id}`
fragment. Every mutating form carries the session CSRF token in a hidden field;
mismatches are rejected. Session cookie: signed, `HttpOnly`, `SameSite=Lax`,
`Secure` when the base URL is HTTPS.

Templates (`base`, `boards`, `board`, `login`, `error`, `_column`, `_card`) and
one hand-written `app.css`. **Mobile-first**: single-column stack by default,
horizontally scrolling flex row of columns at wider breakpoints, touch targets
≥44px, viewport and `theme-color` meta tags, safe-area insets. Serve a real
`manifest.webmanifest`. Semantic HTML with stable `#card-{uuid}` / `.column` /
`.card` hooks. **No service worker, no offline handling** — online-only is a
licence to skip all of it.

`AppError` maps to status via `IntoResponse`: `Forbidden` → 403, `NotFound` →
404, `Domain` → 422 re-rendering the board with the message, `Repo` → 500
logged with the cause and not leaked to the page.

Tests via `tower::ServiceExt::oneshot` with fake ports and dev bypass on:
unauthenticated access to a board redirects to `/login`; creating a board then
fetching it renders the card title; a move POST redirects 303 and the follow-up
GET shows the card in its new column; a POST without a valid CSRF token is
rejected; one user cannot fetch another's board.

## Phase 5 — CI, docs, developer entry point

- GitHub Actions: `fmt --check`, `clippy -D warnings`, `cargo test --workspace`
  on stable; a second job with a `postgres:16` service that sets
  `ANAMNESIS_TEST_PG_URL` so the Postgres contract tests actually run.
- `.env.example` covering every variable in the table above.
- `README.md`: what Anamnesis is, the placeholder caveat, quickstart with
  SQLite + dev bypass, and a worked Authentik configuration example (redirect
  URI, scopes, client type) presented as *an* example, not as the only option.
- `CONTRIBUTING.md`: the core/shell rule, TDD expectation, how to run each test
  layer.
- Confirm `cargo run` with SQLite + dev bypass serves a usable board, and say
  so plainly in the final report — including anything that does not work.
