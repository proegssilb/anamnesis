# Anamnesis dev tasks. Run `just` with no arguments to list them.
# See README.md ("Quickstart", "Running the tests") for the underlying
# commands these wrap.

# Load .env if present (never committed — see .env.example / .gitignore),
# so `just run` picks up real config the same way direnv/dotenvx would.
set dotenv-load := true
set dotenv-filename := ".env"

default:
    @just --list

# Run anamnesis-web. Fills in the SQLite + dev-auth-bypass quickstart
# defaults from README.md for any required variable not already set via
# .env or the environment — never do this bypass in a real deployment.
run:
    #!/usr/bin/env bash
    set -euo pipefail
    export ANAMNESIS_DATABASE_URL="${ANAMNESIS_DATABASE_URL:-sqlite://anamnesis.db?mode=rwc}"
    export ANAMNESIS_BASE_URL="${ANAMNESIS_BASE_URL:-http://localhost:8080}"
    export ANAMNESIS_DEV_AUTH_BYPASS="${ANAMNESIS_DEV_AUTH_BYPASS:-1}"
    export ANAMNESIS_TIMEZONE="${ANAMNESIS_TIMEZONE:-America/New_York}"
    export ANAMNESIS_BOOTSTRAP_ADMIN="${ANAMNESIS_BOOTSTRAP_ADMIN:-dev-user}"
    export ANAMNESIS_SESSION_SECRET="${ANAMNESIS_SESSION_SECRET:-$(openssl rand -hex 64)}"
    cargo run --bin anamnesis-web

# Same as `run`, rebuilding on source changes (requires cargo-watch).
watch:
    #!/usr/bin/env bash
    set -euo pipefail
    export ANAMNESIS_DATABASE_URL="${ANAMNESIS_DATABASE_URL:-sqlite://anamnesis.db?mode=rwc}"
    export ANAMNESIS_BASE_URL="${ANAMNESIS_BASE_URL:-http://localhost:8080}"
    export ANAMNESIS_DEV_AUTH_BYPASS="${ANAMNESIS_DEV_AUTH_BYPASS:-1}"
    export ANAMNESIS_TIMEZONE="${ANAMNESIS_TIMEZONE:-America/New_York}"
    export ANAMNESIS_BOOTSTRAP_ADMIN="${ANAMNESIS_BOOTSTRAP_ADMIN:-dev-user}"
    export ANAMNESIS_SESSION_SECRET="${ANAMNESIS_SESSION_SECRET:-$(openssl rand -hex 64)}"
    cargo watch -x 'run --bin anamnesis-web'

# Full workspace test suite (matches CI's `check` job; Postgres contract
# test is #[ignore]d by default so this stays green with no database running).
test:
    cargo test --workspace

# anamnesis-core only — pure domain logic, #[test] + rstest.
test-core:
    cargo test -p anamnesis-core

# anamnesis-app only — includes the cucumber BDD suite.
test-app:
    cargo test -p anamnesis-app

# anamnesis-adapters — SqlStore contract, SQLite side only.
test-adapters:
    cargo test -p anamnesis-adapters

# anamnesis-adapters — SqlStore contract, Postgres side (needs a running
# Postgres; defaults match .github/workflows/ci.yml's service container).
test-adapters-pg pg_url="postgres://anamnesis:anamnesis@localhost:5432/anamnesis_test":
    ANAMNESIS_TEST_PG_URL="{{pg_url}}" cargo test -p anamnesis-adapters --test sql_store_contract -- --ignored

# anamnesis-web — HTTP integration tests via tower::ServiceExt::oneshot.
test-web:
    cargo test -p anamnesis-web

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Complexity/parameter-count/length check via Lizard, run standalone since
# Codacy's own CLI/MCP can't analyze Rust in this environment (see
# CLAUDE.md). Codacy's function-length limit is 50 *NLOC* — learned from
# PR #17's own annotations, which named two functions at exactly the 55 and
# 89 this invocation reports. `-T nloc=50` is the knob that matches it;
# `-L 50` is not (it thresholds raw length, counting blank and comment
# lines, and over-flags by about a fifth). A fail here is a prompt to look
# at the actual function, not to restructure around this tool's counting
# behavior.
lizard:
    python3 -m lizard -l rust crates -T nloc=50 -w

# The two static-analysis proxies for what Codacy checks on a PR.
quality: clippy lizard

# Everything CI's `check` job runs, plus the Codacy proxies (stricter than
# CI, which doesn't run Lizard yet).
check: fmt-check quality test
