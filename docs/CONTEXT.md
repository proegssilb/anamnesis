# Anamnesis — Project Context

> _ἀνάμνησις_ — recollection; the recovery of knowledge you already had.
>
> "What's below the horizon isn't gone - it's just not up yet."

## What this is

Anamnesis is a personal task tracker. The name and the tagline carry the core
premise: **tasks that are not currently relevant are not deleted and not
nagging — they are below the horizon, and they come back up when they should.**

## Stated product constraints

These come directly from the project owner and are binding on design decisions:

1. **The user loses their position a lot, and that is fine.** Anamnesis is
   built for someone whose attention is interrupted. Losing scroll position,
   losing your place in a flow, coming back after three weeks — all normal,
   none of it is an error state. The system re-orients the user; it does not
   expect the user to hold state in their head.
2. **All authentication is delegated.** Anamnesis never stores a password.
   Login goes to an external OAuth2/OIDC provider — Authentik is the reference
   deployment, but the admin picks. Anamnesis must not special-case Authentik.
3. **The deploying admin picks the database.** SQLite for single-user or small
   deployments, an external SQL server otherwise. Selected by connection
   string, `sqlite://` prefix for SQLite.
4. **The UI must be ready for a future online-only PWA mobile app.** It
   started as server-rendered HTML + CSS with no JavaScript; the owner later
   approved htmx plus a drag library for the task board specifically
   (`docs/DOMAIN.md` §8), with form-POST fallbacks kept everywhere htmx also
   handles. Either way, it must not paint itself into a corner that a mobile
   PWA cannot reuse — no real-time sync, resource-oriented routes throughout.

## Current phase: the real domain model

The placeholder kanban board (`Board`/`Column`/`Card`) served its purpose —
proving the stack end to end — and has been fully replaced. The real domain
model described in `docs/DOMAIN.md` (areas, projects, tasks, the horizon
placement, tangles, the capacity-gated suggestion engine, recurrence and
sweeping) is now built, running, and covered at every test layer; see
`docs/ARCHITECTURE.md` for how it's structured and `README.md` for what
still doesn't work yet. A few things from the placeholder phase remain true:

- The **domain core is still the asset** that mattered most — the specific
  shape of any one feature is more disposable than the boundary between
  core and the outside world.
- **No MCP server yet.** Still anticipated, still not built.

## Provenance — read this before trusting the above

This file is a **stub, pending transcription.** The project owner's actual
accumulated context for Anamnesis lives in Claude Chat memories, not here and
not in Claude Code; they will have those transcribed into this repository
later.

What is written above came from two sources only, on 2026-08-26: the repository
README, and a direct briefing from the owner at the start of the scaffolding
work. The session that wrote it had no access to the Chat memories — no memory
tool was exposed to Claude Code, and the repository had no issues, wiki, or
project board to read.

So: when the transcribed memories land, they are **authoritative and this file
yields to them.** Expect them to add history and intent this stub cannot have,
and expect them to contradict it in places. Merge rather than append, and
delete this notice once the real context is in.
