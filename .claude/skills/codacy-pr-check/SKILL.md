---
name: codacy-pr-check
description: Find and fix real Codacy findings on an open PR when Codacy's own PR comment and dashboard don't give you enough to act on. Use whenever a PR's Codacy check is red/action_required, or "Codacy still complains" after `just quality` came back clean.
---

# Checking a PR for Codacy issues

Codacy's PR comment (posted by `codacy-production[bot]`) is aggregate-only —
category counts like "3 medium Complexity, 1 medium BestPractice" with a
link to `app.codacy.com` — never a file or line. Do not spend time trying
to reach that link or Codacy's own API: `app.codacy.com` and
`api.codacy.com` are network-blocked from this environment's egress proxy,
and every CONNECT to them will be rejected. This is a known, permanent
limitation here, not a transient failure worth retrying.

## Get the real file/line detail

The actual findings — file, line, and the specific rule violation — live in
the GitHub Check Run's *annotations*, not in its `output.summary` (which is
the same aggregate text as the PR comment) and not in any inline PR review
comment (Codacy doesn't leave those on this repo). Pull them directly from
GitHub's REST API, which has no dedicated wrapper in the GitHub MCP
toolset:

```bash
curl -sS "https://api.github.com/repos/<owner>/<repo>/check-runs/<check_run_id>/annotations"
```

1. Find the "Codacy Static Code Analysis" check run's id for the PR's head
   commit — `pull_request_read` → `get_check_runs` lists it (or use
   `get_check_run` if a webhook event already gave you the id).
2. Call the annotations endpoint above with that id. Each entry has `path`,
   `start_line`/`end_line`, and a `message` naming the actual violation,
   e.g. `Method assemble has 9 parameters (limit is 8)` or
   `Unexpected empty function.`
3. `curl` against `api.github.com` works directly in this environment (the
   proxy authenticates it transparently) even though direct GitHub API
   access is otherwise off-limits per the session's GitHub Integration
   instructions — this one read-only endpoint is the exception because
   nothing else exposes it. If it ever 403s, there's no fallback short of
   asking whoever has the Codacy dashboard open to paste the finding list.

Codacy only reports **new** issues — ones on lines the PR's diff touches —
so a pre-existing violation elsewhere in a file you're editing won't show
up here even if it trips the same rule.

## Fixing what you find

Follow this repo's `CLAUDE.md` "Codacy" section: fix the real design
issue, don't game the metric. Two things specific to this workflow:

- `just quality` (clippy + lizard) is a *proxy*, not a subset of what
  Codacy checks — it can be clean while Codacy still flags something, and
  the reverse can also mislead you: an existing
  `#[allow(clippy::too_many_arguments)]` silences clippy locally, but
  Codacy's own parameter-count rule doesn't respect Rust's `#[allow]`
  attributes at all, so a function can look clean under `just quality` and
  still get flagged as a *new* issue the moment the PR's diff touches it.
- `just lizard` only scans Rust (`-l rust crates`). If an annotation points
  at a `.js` file, check it with lizard directly:
  `python3 -m lizard -l javascript <path> -T nloc=50 -C 10 -w`.

When an annotation is a parameter-count finding, only bundle the
parameters into a struct if they're genuinely one cohesive concept (a DB
row, a request payload, a template's render context) — not as a wrapper
purely to change how the linter counts. See `CLAUDE.md`'s `9e6be4d`
cautionary example for what that looks like done wrong.

Before pushing: `cargo build --workspace --all-targets`, `just quality`
(or the equivalent `cargo clippy --workspace --all-targets -- -D
warnings` + `python3 -m lizard -l rust crates -T nloc=50 -C 10 -w`),
`cargo fmt --all -- --check`, and `cargo test --workspace`. After pushing,
confirm on the PR itself that Codacy's comment updates to "Up to
standards" / `0 issues` — don't take a clean local run as proof by itself.
