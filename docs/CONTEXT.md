# Anamnesis — Project Context

> *ἀνάμνησις* — recollection; the recovery of knowledge you already had.
>
> "What's below the horizon isn't gone - it's just not up yet."

## What this project is

Anamnesis is a personal task tracker. The name and the tagline carry the core
premise: **tasks that are not currently relevant are not deleted and not
nagging — they are below the horizon, and they come back up when they should.**

## What this file is

This file attempts to compile information from multiple raw chats with Claude
and Claude Code.

The Origin and Design-decisions sections were transcribed from the project
owner's Claude Chat history on 2026-09-05; that history, not this file, is
the original record. Said history can date back to 2026-08-26 or older. The
product constraints below predate the chat history transcription, from a
direct briefing on 2026-08-26. Note the overlap in time; information was
compiled from both sources, and the overlap required some editorial discretion.

While the origin of the information is straightforward, the present state of
this file is less so. Preserving all the original context exactly as it was
discussed would create contradictory information; at one point, there was a
"no JavaScript anywhere" policy that failed to create a better user experience
than using JavaScript judiciously. That policy was therefore abandoned. In
order to keep this document from misleading readers about current state, some
history has been pruned or revised as a judgement call. The goal is to provide
useful perspective, not to endlessly document irrelevant history.

## Stated product constraints

These come directly from the project owner and are binding on design decisions:

1. **All authentication is delegated.** Anamnesis never stores a password.
   Login goes to an external OAuth2/OIDC provider — Authentik is the reference
   deployment, but the admin picks. Anamnesis must not special-case Authentik.

2. **The deploying admin picks the database.** SQLite for single-user or small
   deployments, an external SQL server otherwise. Selected by connection
   string, `sqlite://` prefix for SQLite.

3. **The UI must be ready for a future online-only PWA mobile app.** It
   started as server-rendered HTML + CSS with no JavaScript; the owner later
   approved htmx plus a drag library for the task board specifically
   (`docs/DOMAIN.md` §8), with form-POST fallbacks kept everywhere htmx also
   handles. Either way, it must not paint itself into a corner that a mobile
   PWA cannot reuse — no real-time sync, resource-oriented routes throughout.

## Current phase: MVP

v1.0 has been released. It's functional, and you can run it in production,
but it's an early pass and some of that immaturity probably shows.

## Origin

Anamnesis started from a straightforward frustration with existing task
trackers: they either nag constantly about everything at once, or let things
vanish into a backlog that never gets revisited. The goal was a system built
around externalizing tasks — putting something down with real trust that it
will come back at the right moment, rather than either holding it in working
memory or losing it for good.

An existing self-hosted tool was evaluated as a possible foundation and
rejected. Its schema only models containment (`parent_task_id`), not a true
blocking relationship between tasks, and bolting dependency- and
capacity-aware resurfacing onto someone else's data model wasn't going to
fit as well as building it in from the start.

The project was originally named "Lethe," after the river of forgetting. It
became Anamnesis — recollection; also, fittingly, the term for a patient's
history-taking in a clinical context — because a tool whose entire value is
"relax, and this will come back to you when it's time" deserves to be named
for the return trip rather than the departure.

## Design decisions and their reasoning

A few decisions were deliberate or contested enough that a later contributor
(human or AI) is likely to want to revisit them. Recorded here so they aren't
relitigated from scratch:

- **Tangles don't get a work-in-progress exemption.** A "tangle" (an isolated
  cluster of tasks containing one or more cycles) was proposed to sit outside
  WIP accounting entirely, with a dedicated `Stuck` state, to head off a
  theoretical livelock where a full board goes silent forever. That was
  rejected: tangled tasks aren't suggestion-eligible in the first place, so a
  full board is realistically full of ordinary completable work that drains
  normally — and the user always has manual override to pick up and complete
  any task directly regardless of what the suggestion engine offers. A full
  board offering nothing is expected, self-explanatory behavior, not a UX
  failure requiring a special state. The one accepted refinement: if capacity
  is open but everything left to suggest is tangled, the suggester should say
  so rather than staying quietly silent — a status line, not an architectural
  carve-out.

- **MCP tokens, once the surface exists, should carry narrower capabilities
  than a full user session.** Anamnesis is a trust-based tool handed to an
  agent; giving an MCP client the same scope as the human's own session
  creates a confused-deputy risk. This is a constraint for whoever builds the
  MCP surface, not yet enforced since the surface doesn't exist.

## Where the rest of the context lives

Domain model detail — areas, projects, tasks, horizon placement, tangles, the
capacity-gated suggestion engine, recurrence, sweeping — lives in
`docs/DOMAIN.md`. System structure lives in `docs/ARCHITECTURE.md`. This file
is for intent and history those documents don't carry: why a decision was
made, not what it produced.
