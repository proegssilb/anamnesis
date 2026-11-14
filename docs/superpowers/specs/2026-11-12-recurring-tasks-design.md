# Recurring tasks — design

Issue #37. Status: draft for review.

## Intent

Household chores — taking out the trash, replacing furnace filters,
vacuuming — should come back on their own, so they don't have to be carried
in the user's head. That is Anamnesis's premise ("what's below the horizon
isn't gone — it's just not up yet") applied to things that repeat.

Chores come in two kinds, and the design is built around the difference:

- **Deadline chores** (trash). Tied to the calendar and to a time of day.
  Missing the moment makes the task pointless until next time. These *must*
  reach the board, even when the board is full.
- **Interval chores** (furnace filter, vacuuming). Loosely due; usually
  counted from when they were last done. A few days late is fine. These go
  through the suggestion engine, but should come up more often than not
  once due.

Issue #37 flags the risk: recurring tasks are "highly prone to WIP limit
violations." The answer here is that only deadline chores may exceed the
limit, and only because the user genuinely does have one more thing to do
that evening.

## Goals

- A recurring task is described by a short typed phrase ("every tue at 6pm
  until 8am", "3 months after done") with an immediate preview of the next
  few dates. The phrase is the usability layer; a structured `Schedule`
  value in core is the source of truth.
- Schedules can be calendar-anchored or counted from last completion.
- Schedules can say: every N days / weeks / months; a set of weekdays; a
  day of the month (clamped, as the sweep already does); the last day of the
  month; the Nth or last weekday of the month.
- A schedule with a time of day ("at 6pm") makes it a **deadline chore**: at
  that local time it is placed onto the board's entry column regardless of
  the WIP limit.
- A schedule without one is an **interval chore**: when due it is placed
  below the horizon and gets strongly preferred, cooldown-exempt treatment
  from the suggestion engine.
- A recurring task is **one task that cycles**, not a template that stamps
  out copies. Its comments, checklist, fields and history stay with it.

## Non-goals

- Changing the archive sweep's own `Recurrence` type or settings UI. It
  stays as-is; the two may converge later.
- Full RFC 5545 RRULE compatibility, calendar import/export.
- Natural-language parsing beyond the documented grammar below. The
  grammar is small and exact; unrecognised input is an error with a hint,
  not a guess.
- Notifications/push. A page refresh remains the update mechanism
  (`DOMAIN.md` §8).

## Anti-goals (would make this a failure even if it works)

- **Nagging.** No "you missed the trash" banners, no overdue badges in red,
  no counters of missed occurrences. A missed deadline chore quietly leaves.
- **Soft chores breaking "Full is silence."** Only deadline chores may push
  onto a full board. An interval chore never appears when the board is full.
- **Template sprawl.** No pile of archived "Take out trash (week 37)"
  copies.
- **Bounce pollution.** Automatic movements (a deadline chore expiring back
  below) must not count as bounces; bounce count measures the user's
  resistance, not the clock.

## Approaches considered

- **A. Hand-built `Schedule` type in core** — chosen as the model. Fits the
  pure, dependency-light core (`serde`, `thiserror`, `time`, `uuid` only),
  extends the existing local-calendar `next_run` approach and its edge-case
  tests (DST, month-end). Trade-off: expresses what we design, not
  everything.
- **B. RFC 5545 RRULE via the `rrule` crate** — rejected. Brings `chrono` and
  a second tz database into core, needs a supply-chain review, unfriendly to
  type, and has no native "after completion" anchor.
- **C. A small text grammar** — chosen as the input layer on top of A, at
  the user's request: "exactly the kind of usability touch Anamnesis kinda
  needs."

The user picked **A as the core with C on top**, and added a pop-in time of
day for deadline chores.

## Design

### Core model (`anamnesis-core`, new module `schedule.rs`)

```rust
pub struct Schedule {
    pub rule: Rule,
    pub anchor: Anchor,
    pub pop_in: Option<PopIn>,   // Some => deadline chore
}

pub enum Rule {
    Days   { n: u16 },
    Weeks  { n: u8, on: WeekdaySet },      // empty set only valid with AfterCompletion
    Months { n: u8, on: MonthDay },        // `on` ignored with AfterCompletion
}

pub enum MonthDay {
    Day(u8),                               // 1..=31, clamped to month length
    LastDay,
    Nth { nth: Nth, weekday: Weekday },    // Nth = First..Fourth | Last
}

pub enum Anchor {
    Calendar { start: Date },              // phase for n > 1; see below
    AfterCompletion,
}

pub struct PopIn {
    pub at: time::Time,                    // local wall-clock time
    pub until: Option<time::Time>,         // local time it expires; see Expiry
}
```

`WeekdaySet` is a 7-bit set. All types derive `Serialize`/`Deserialize`.

Pure functions, local-calendar only, exactly like `recurrence.rs` (timezones
remain the caller's job via `TimezoneResolver`):

- `next_occurrence(schedule, after: Date, last_completed: Option<Date>) -> Date`
  — the next due local date strictly after `after`. For `AfterCompletion`,
  it is `last_completed + interval` (or, if never completed, the date the
  schedule was set, i.e. due now).
- `parse(input: &str) -> Result<Schedule, ParseError>` and
  `describe(&Schedule) -> String` (canonical phrase; `parse(describe(s)) == s`).

**Calendar phase.** For `n > 1` ("every 2 weeks on tue"), the on-cycle week
is the one containing `start`, where `start` is the first matching date on
or after the day the schedule was saved. This differs on purpose from the
sweep's fixed-epoch anchor: for a task, "every other Tuesday starting this
coming Tuesday" is what a person means. The sweep's epoch rationale (no
drift when the result is fed back as `from`) still holds because `start` is
stored, not recomputed.

### Grammar (the C layer)

Case-insensitive, whitespace-tolerant. Weekdays accept `mon`…`sun`, full
names, plurals ("tuesdays"), `weekday`/`weekdays`, `weekend`.

```
schedule   := cadence [ "at" time [ "until" time ] ]
cadence    := "every" [N] unit [ "on" on-spec ]     -- calendar
            | "every" weekday-list                  -- = every 1 week on …
            | N unit "after done"                   -- after completion
            | "every" [N] unit "after done"
unit       := day(s) | week(s) | month(s)
on-spec    := weekday-list                          -- weeks
            | "the" ordinal-day                     -- months: "the 15th"
            | "the last day"
            | "the" nth weekday                     -- "the last fri", "the 2nd sat"
time       := 6pm | 6:30pm | 18:00 | noon | midnight
```

Examples that must round-trip:

| Phrase | Meaning |
|---|---|
| `every tue at 6pm until 8am` | trash: board at Tue 18:00, expires Wed 08:00 |
| `3 months after done` | furnace filter |
| `every week after done` | vacuuming |
| `every mon, thu` | interval chore, calendar-anchored |
| `every 2 weeks on sat` | every other Saturday from the next one |
| `every month on the last fri` | |
| `every month on the 31st` | clamps in short months |
| `every 10 days` | |

Errors name the offending word and suggest the nearest valid form
("didn't understand 'fortnight' — try 'every 2 weeks'").

### Task state

New columns on `tasks` (migration `0008_recurring_tasks.sql`, SQLite and
Postgres):

- `schedule_json TEXT NULL` — serde JSON of `Schedule`; `NULL` = not recurring.
- `next_due_at` timestamp `NULL` — UTC instant of the next pop-in (deadline
  chore) or local-midnight of the next due date (interval chore). Indexed;
  the ticker queries `next_due_at <= now`.
- `expires_at` timestamp `NULL` — set while a deadline chore is up and has
  an `until`.
- `last_completed_at` timestamp `NULL`.

`Task` in core gains the same four fields (schedule parsed). JSON rather
than discrete columns because the shape is nested and only core interprets
it; `next_due_at` is the only thing SQL needs to filter on.

### Lifecycle

1. **Set / change schedule** (task page): parse, store, compute
   `next_due_at` from today. Clearing it nulls all four fields.
2. **Completion**: any move into an `is_done` column (both
   `use_cases::task` move paths *and* `board::reposition_board_item`)
   stamps `last_completed_at` and, for `AfterCompletion`, recomputes
   `next_due_at`. The existing archive sweep may archive it as normal.
3. **Due** (ticker, below): for each recurring task with
   `next_due_at <= now`:
   - Wherever it is (archived, in Done, below), un-archive it and:
     - **deadline chore** → place at the end of the board's entry column,
       **bypassing `check_wip_limit`**; set `expires_at` from `until`.
     - **interval chore** → place `Below`, mark it *due* (see Suggestions).
   - If it is currently on the board and *not* done (the user already has
     it up), leave its placement alone.
   - Advance `next_due_at`: calendar → next occurrence after today;
     after-completion → leave as-is (it stays due until completed).
   - **Reset its checklist**: every checklist item (task with this
     `parent_task_id`) that is archived or in an `is_done` column is
     un-archived and placed `Below`. Items not yet done are left where they
     are. (User's decision.)
   - Do not touch `bounce_count`, `last_offered_at` or `last_touched_at`
     (of the task or its checklist items).
4. **Expiry** (deadline chores): when `expires_at <= now` and the task is
   still on the board and not in a done column, move it `Below` *without*
   bouncing, clear `expires_at`. No message. If no `until` was given, it
   expires when the next occurrence comes due (i.e. it stays up; the next
   firing is a no-op on placement).

Catch-up after downtime: like the sweep, due-ness is computed from stored
timestamps, so missed firings run on startup. Multiple missed calendar
occurrences collapse into one firing (no backlog of copies — it's one task).
A deadline chore whose `expires_at` is already past at catch-up time is
skipped straight to the next occurrence rather than shown stale.

### Suggestion engine

`TaskSummary` gains `due_since: Option<Timestamp>` (set for an interval
chore that is due and below the horizon; cleared on completion).

- Due chores are **exempt from cooldown** (`is_eligible`). They still
  require: not archived, below, Active project, unblocked, untangled.
- **Due slot.** When the offer has ≥1 slot and at least one eligible due
  chore exists, a seeded draw with probability `due_slot_probability`
  (new `SuggestionSettings` field, default **0.8**) gives the first slot to
  a due chore, picked by weight growing with how long it's been due. New
  `SuggestionReason::Due`. Otherwise normal composition. Seed stability is
  unchanged, so refreshing does not re-roll.
- `Full` remains silence; a due interval chore never pushes onto a full
  board.

### Ticker

The sweep ticker polls daily, which is too coarse for "at 6pm". Add a
separate `recurrence_ticker.rs` in `anamnesis-web`, spawned only from
`main.rs` (same structural test isolation as `sweep.rs`), using a
`job-lease` lease so only one instance fires. It sleeps until the earliest
of `min(next_due_at)`, `min(expires_at)`, or a ceiling (15 minutes, so a
schedule edited in another instance is picked up). Pure `due_tasks(now, …)`
/ `expired_tasks(now, …)` decisions are unit-tested without the ticker.

### UI

- Task page: a "Repeats" section with **two linked editors for the same
  `Schedule`** (user's decision): the typed phrase, and a click-through
  builder of dropdowns for people who'd rather pick than type.

  ```
  Repeats: [ every tue at 6pm until 8am            ]   ← phrase

  Every [ 1 ] [ week(s) ▾ ]  counted from [ the calendar ▾ ]
  On    [x]Mon [ ]Tue ... [ ]Sun                     ← weeks
        [ the 15th ▾ ] / [ last day ] / [ 2nd ▾ ][ fri ▾ ]  ← months
  [x] Put it on the board at [ 6 ▾ ]:[ 00 ▾ ][ pm ▾ ]
      [x] and take it back down at [ 8 ▾ ]:[ 00 ▾ ][ am ▾ ]

  Next: Tue Nov 17 6:00pm · Tue Nov 24 6:00pm · Tue Dec 1 6:00pm
  ```

  Fields show/hide by unit (weekday checkboxes for weeks, day-of-month
  choices for months, nothing for days; the "on" row hides for "after
  done"). **The server is the only interpreter**: a change in either editor
  posts the form via htmx; the handler builds the `Schedule` (with
  `parse` for the phrase, or directly from the dropdown fields) and
  re-renders the whole section — canonical phrase, dropdowns set to match,
  and the three-date preview — or an inline error. No client-side parser,
  so the two can never disagree. Save stores it; clearing clears it. Works
  without JS by submitting the form (the phrase field wins if both
  changed).
- Card: a small, quiet repeat glyph on recurring tasks. No overdue styling.
- Suggestion copy for `SuggestionReason::Due`: neutral ("due again"), never
  scolding.

### Testing

- Core: table tests for `parse`/`describe` round-trip; `next_occurrence`
  over month-end, leap years, Nth/last weekday, n>1 phase, after-completion.
- Core: suggestion engine — due chore ignores cooldown; due slot fires at
  the configured probability over many seeds; `Full` unaffected.
- App: cucumber feature `recurring_tasks.feature` — trash pops onto a full
  board; expires below without a bounce; filter recomputes from completion;
  catch-up after downtime collapses missed occurrences.
- Adapters: store contract test for the new columns on both databases.
- Grounds rule: DST — a pop-in time that doesn't exist on spring-forward day
  resolves via `TimezoneResolver`'s existing policy; test it.

## Decided by the user

- Chores are the motivating use case.
- Deadline chores always reach the board, even over the WIP limit.
- Interval chores skip the cooldown and should come up more often than not.
- Model = A (structured core type); input = C (typed grammar with preview);
  plus a pop-in time of day.
- Alongside the typed phrase, a dropdown builder to click through the same
  schedule; the two stay in sync.
- When a recurring task comes due, its finished checklist items reset to
  not-done (back below the horizon).
- Recurring tasks get no exemption from project status. Chores live in an
  ordinary project that holds an Active slot like any other ("chores are
  just one of the things you have to carry in this life"); the limit is
  already configurable on `/settings`. A task in a non-Active project does
  not fire at all — deadline chores included — and its `next_due_at` is left
  pending so it fires on the first tick after the project is reactivated
  (collapsed to one occurrence, as with downtime catch-up).

## Claude's calls (check these)

- Having a time of day is what makes a chore a deadline chore; there's no
  separate "hard/soft" toggle.
- One task cycles; no copies.
- `until` expiry for deadline chores; without `until`, it stays up until
  done.
- Expiry and firing never count as bounces.
- "More often than not" = 80% chance the first offer slot goes to a due
  chore, tunable.
- Calendar `n > 1` phases from the first occurrence after saving, not the
  epoch.
- Calendar chores ignore early completion (do the vacuum Thursday, a
  Saturday calendar schedule still fires Saturday) — use "after done" for
  chores where that matters.
- JSON column for the schedule; separate ticker with a 15-minute ceiling.
- Archiving a recurring task explicitly (the task page's Archive action)
  stops it recurring (clears `next_due_at`); the sweep and "Archive all"
  don't — they just tidy Done, and the task returns on schedule. Restoring
  it recomputes `next_due_at` from today.
- A recurring task linked to a GitHub/Forgejo issue gets no special
  handling; sync sees ordinary placement changes.

## Left to the builder

Exact module/file layout, grammar parser technique (hand-written; no parser
crate), template markup, error wording, and the weight curve for the due
slot.
