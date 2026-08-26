# anamnesis
What's below the horizon isn't gone - it's just not up yet.

_ἀνάμνησις_ — recollection; the recovery of knowledge you already had.

## What this is

Anamnesis is a personal task tracker, built on the premise that tasks which
aren't currently relevant shouldn't be deleted and shouldn't nag — they sit
below the horizon and resurface when they should. It's designed for someone
whose attention is interrupted: losing your scroll position, losing your
place in a flow, coming back after three weeks — all normal, none of it
treated as an error state.

See `docs/CONTEXT.md` for the fuller product framing and its provenance.

## This repo is a placeholder — read this before judging the feature set

**The kanban board implemented here is disposable scaffolding, not the
product.** It exists to prove one thing: that the architecture (a pure
functional core, OIDC-delegated auth, SQLite-or-Postgres persistence, a
server-rendered PWA-ready shell) actually works end to end, with real tests
at every layer.

It is explicitly **not** the real product model. The real Anamnesis is a task
tracker with dependency management, areas, and projects — none of which
exist yet. Once this scaffold is proven, expect the domain model, routes,
and templates to be **massively reorganized**. Don't build on top of the
kanban shape; the boundaries between layers (see `docs/ARCHITECTURE.md`) are
the part meant to survive that reorganization, not the columns-and-cards UI.

## Quickstart (SQLite + dev auth bypass)

This is the fast path for running Anamnesis locally with no external identity
provider — every request is authenticated as a fixed local user. **Never do
this in a real deployment**; see the Authentik section below for real auth.

### Prerequisites

- Rust (stable toolchain — edition 2024, so a reasonably recent `rustc`;
  install via [rustup](https://rustup.rs) if you don't have one)
- `openssl` (or any way to generate a 64+ byte random string) for the session
  secret
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

# 3. Run it.
cargo run --bin anamnesis-web
```

You'll see a loud warning that dev auth bypass is on (expected), then:

```
INFO anamnesis_web: anamnesis listening addr=0.0.0.0:8080
```

Open **http://localhost:8080/boards** in a browser — you're already "logged
in" as a fixed dev user, and can create a board, add columns, add cards, and
move cards between them. `GET /healthz` returns `200 ok` for a quick check
that the process is alive.

`.env.example` in the repo root documents every variable, including the
ones this quickstart skips (`ANAMNESIS_BIND_ADDR`, the `ANAMNESIS_OIDC_*`
family, `ANAMNESIS_OIDC_SCOPES`). Anamnesis reads the process environment
directly — it does not load `.env` itself — so use `direnv`, `dotenvx run`,
or export the variables as shown above.

This exact sequence (fresh SQLite file, dev bypass, `cargo run`, `curl
/healthz`, `curl /boards`, create a board via `POST /boards`, follow the
redirect) was run against this README while writing it, from an empty
working directory, to confirm it actually works as written.

## Real authentication: OpenID Connect

Anamnesis never stores a password. Login is OAuth2 Authorization Code + PKCE
against **any OIDC-compliant provider**, discovered from its issuer URL
(`/.well-known/openid-configuration`). There is no provider-specific code in
Anamnesis — if you ever find a branch that special-cases one identity
provider, that's a bug.

### Worked example: Authentik

[Authentik](https://goauthentik.io/) is the reference deployment used while
building this scaffold — it is **one example provider among any that speak
standard OIDC**, not a requirement.

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
   ```

4. `cargo run --bin anamnesis-web`, then visit `/login` — you'll be
   redirected to Authentik, and back to `/auth/callback` on success.

Any other OIDC provider (Keycloak, Zitadel, Okta, Auth0, Google Workspace,
...) works the same way: register a confidential OAuth2/OIDC client, set its
redirect URI to `${ANAMNESIS_BASE_URL}/auth/callback`, and point
`ANAMNESIS_OIDC_ISSUER_URL` at its issuer.

## Running the tests

Each layer of `docs/ARCHITECTURE.md`'s hexagon has its own test tool, testing
something the others can't.

```sh
# Everything except the Postgres contract test (which is #[ignore]d by
# default so `cargo test` stays green with no database running):
cargo test --workspace

# anamnesis-core only — pure domain logic, #[test] + rstest, no fakes needed:
cargo test -p anamnesis-core

# anamnesis-app only — includes the cucumber BDD suite (feature files in
# crates/anamnesis-app/features/), driving real use cases against in-memory
# fakes. Wired as a `[[test]] harness = false` target, so it runs as part of
# the normal `cargo test` for this crate too:
cargo test -p anamnesis-app

# anamnesis-adapters — the SqlBoardRepository contract. SQLite runs against
# a temp file automatically. Postgres is gated behind an env var and
# #[ignore]d otherwise:
cargo test -p anamnesis-adapters                     # SQLite side only
ANAMNESIS_TEST_PG_URL="postgres://user:pass@localhost/db" \
  cargo test -p anamnesis-adapters --test board_repository -- --ignored   # Postgres side

# anamnesis-web — HTTP integration tests via tower::ServiceExt::oneshot
# (routing, forms, redirects, auth gating, CSRF, status codes), no socket:
cargo test -p anamnesis-web
```

Before committing, also run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

CI (`.github/workflows/ci.yml`) runs all of the above on every push and pull
request, including the Postgres contract test against a real `postgres:16`
service container.

## Architecture

Imperative Shell / Functional Core across four crates (`anamnesis-core` →
`anamnesis-app` → `anamnesis-adapters` → `anamnesis-web`), with dependencies
pointing one way only. The full write-up — the dependency rule, the port
traits, the persistence and auth design, and the testing strategy — lives in
`docs/ARCHITECTURE.md`. Read that before changing anything structural; this
README stays a pointer to it rather than a second copy.
