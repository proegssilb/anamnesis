# Recurring Tasks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tasks (household chores) that come back on their own on a typed or click-built schedule — deadline chores onto the board at a set time even over the WIP limit, interval chores into the suggestion engine with cooldown-exempt priority.

**Architecture:** A pure `Schedule` value type, grammar, and per-task lifecycle in `anamnesis-core`; five nullable task columns plus one settings column in a new migration; app use cases that fire/expire due tasks and stamp completion; a dedicated lease-coordinated ticker in `anamnesis-web`; an htmx "Repeats" section on the task page whose phrase and dropdown editors are both interpreted only by the server.

**Tech Stack:** Rust (build with `cargo +1.98`), `time` 0.3, sqlx (SQLite + Postgres), axum + minijinja 3 + htmx, cucumber-rs for `anamnesis-app` features.

**Spec:** `docs/superpowers/specs/2026-11-12-recurring-tasks-design.md`

## Global Constraints

- `anamnesis-core` dependencies stay exactly `serde`, `thiserror`, `time`, `uuid`. No parser crate, no tz crate, no `rrule`/`chrono`.
- No new third-party dependencies anywhere unless ≥14 days old (renovate `minimumReleaseAge`); this plan needs none.
- Core works in local calendar terms only; UTC↔local conversion goes through `anamnesis_app::ports::TimezoneResolver` with `state.timezone_name`.
- Firing and expiry never change `bounce_count`, `last_bounced_at`, `last_offered_at`, or `last_touched_at`.
- Only deadline chores (schedule has a pop-in time) may exceed a WIP limit. `Outcome::Full` stays silence.
- No scolding UI: no overdue colouring, missed counters, or "you missed it" messages. Suggestion copy for due chores: "due again".
- Due-slot probability default **80%**, stored as `suggestion_due_slot_percent` (0–100).
- Ticker sleep ceiling **15 minutes**.
- Every `git commit` is preceded by `just quality` (clippy `-D warnings`, lizard `-T nloc=50 -C 10`). Fix flagged complexity for real; don't reshape code to dodge the counter (see `CLAUDE.md`).
- Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never push or open PRs unless the user asks.

## Review Focus

1. **Pop-in time in a DST spring-forward gap** (e.g. `at 2:30am` on the changeover day) — expected: fires at the first valid instant after the gap, never errors or skips the week. Test in Task 6.
2. **Server down across several occurrences, or past a deadline chore's `until`** — expected: one firing, stale deadline window skipped straight to the next occurrence, no duplicate placement. Test in Task 6.
3. **Schedule edited/cleared while the task is due or on the board** — expected: `next_due_at` recomputed from today, `due_since` cleared, placement untouched. Test in Task 6.
4. **User drags an up deadline chore back below before its `until`** — expected: that drop is a normal (bounce-counting) user action, and the later expiry is a no-op. Test in Task 3.
5. **Nonsense or out-of-range schedule input** (`every 0 days`, `on the 32nd`, `at 25:00`, `every 2 weeks on` with no days, `the 5th fri`, empty string) — expected: a `ParseError` naming the word, never a panic. Test in Task 2.

---

### Task 1: `Schedule` model and occurrence arithmetic

**Files:**
- Create: `crates/anamnesis-core/src/schedule/mod.rs`
- Modify: `crates/anamnesis-core/src/lib.rs` (add `mod schedule;` and re-exports)
- Test: unit tests in `crates/anamnesis-core/src/schedule/mod.rs` (`#[cfg(test)] mod tests`, `rstest` tables as `recurrence.rs` does)

**Interfaces:**
- Produces (all `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`; `Copy` where fields allow):
  - `pub struct Schedule { pub rule: Rule, pub anchor: Anchor, pub pop_in: Option<PopIn> }`
  - `pub enum Rule { Days { n: u16 }, Weeks { n: u8, on: WeekdaySet }, Months { n: u8, on: MonthDay } }`
  - `pub enum MonthDay { Day(u8), LastDay, Nth { nth: Nth, weekday: Weekday } }`
  - `pub enum Nth { First, Second, Third, Fourth, Last }`
  - `pub enum Anchor { Calendar { start: Date }, AfterCompletion { set_on: Date } }` — `set_on` is the save date, used as "due now" when never completed.
  - `pub struct PopIn { pub at: time::Time, pub until: Option<time::Time> }`
  - `pub struct WeekdaySet(u8)` with `new() / with(Weekday) / contains(Weekday) / is_empty() / iter()` (bit 0 = Monday).
  - `pub enum ScheduleError` (thiserror): `ZeroInterval`, `NoWeekdays`, `DayOutOfRange(u8)`.
  - `impl Schedule { pub fn validate(&self) -> Result<(), ScheduleError>; pub fn is_deadline(&self) -> bool }`
  - `pub fn first_start(rule: &Rule, today: Date) -> Date` — first date on/after `today` matching `rule` (the `Calendar { start }` value to store).
  - `pub fn next_occurrence(schedule: &Schedule, after: Date, last_completed: Option<Date>) -> Date`
  - `pub fn upcoming(schedule: &Schedule, after: Date, last_completed: Option<Date>, count: usize) -> Vec<Date>`

- [ ] **Step 1: Write failing tests**

```rust
#[rstest]
// Calendar weekly, n=1, Tue: strictly after a Tuesday is the next Tuesday.
#[case(weekly(1, &[Tue], d(2026,11,17)), d(2026,11,17), None, d(2026,11,24))]
// n=2 phases from the week containing `start`, Monday-based weeks.
#[case(weekly(2, &[Sat], d(2026,11,21)), d(2026,11,22), None, d(2026,12,5))]
// Multi-day set.
#[case(weekly(1, &[Mon, Thu], d(2026,11,16)), d(2026,11,16), None, d(2026,11,19))]
// Every 10 days from start.
#[case(days(10, d(2026,11,1)), d(2026,11,5), None, d(2026,11,11))]
// Day 31 clamps in short months, leap-aware.
#[case(monthly(1, MonthDay::Day(31), d(2026,1,31)), d(2026,1,31), None, d(2026,2,28))]
#[case(monthly(1, MonthDay::Day(31), d(2028,1,31)), d(2028,1,31), None, d(2028,2,29))]
#[case(monthly(1, MonthDay::LastDay, d(2026,4,30)), d(2026,4,30), None, d(2026,5,31))]
// Last Friday / 2nd Saturday.
#[case(monthly(1, nth(Nth::Last, Fri), d(2026,11,27)), d(2026,11,27), None, d(2026,12,25))]
#[case(monthly(1, nth(Nth::Second, Sat), d(2026,11,14)), d(2026,11,14), None, d(2026,12,12))]
// Every 3 months on the 15th from a Nov start skips Dec, Jan.
#[case(monthly(3, MonthDay::Day(15), d(2026,11,15)), d(2026,11,15), None, d(2027,2,15))]
// After completion: 3 months after done, month-end clamped.
#[case(after(months(3)), d(2026,11,30), Some(d(2026,11,30)), d(2027,2,28))]
#[case(after(weeks(1)), d(2026,11,13), Some(d(2026,11,10)), d(2026,11,17))]
fn next_occurrence_cases(#[case] s: Schedule, #[case] after: Date, #[case] done: Option<Date>, #[case] want: Date) {
    assert_eq!(next_occurrence(&s, after, done), want);
}

#[test]
fn after_completion_never_completed_is_due_on_set_on() { /* set_on 2026-11-13, after 2026-11-12 → 2026-11-13 */ }
#[test]
fn feeding_result_back_steps_exactly_one_period() { /* weekly(2,&[Sat]) chained 5x: each diff == 14 days */ }
#[test]
fn upcoming_returns_count_strictly_increasing() { /* upcoming(.., 3).len()==3, windows(2) all a<b */ }
#[rstest]
#[case(days(0, ..), ScheduleError::ZeroInterval)]
#[case(weekly(1, &[], ..), ScheduleError::NoWeekdays)]
#[case(monthly(1, MonthDay::Day(32), ..), ScheduleError::DayOutOfRange(32))]
fn validate_rejects(..) {}
#[test]
fn first_start_finds_next_matching_date() { /* weekly Tue, today Fri 2026-11-13 → 2026-11-17 */ }
```

(`weekly/days/monthly/nth/after/months/weeks/d` are local test helpers building `Schedule`s with `pop_in: None`.)

- [ ] **Step 2: Run to verify failure** — `cargo +1.98 test -p anamnesis-core schedule` → compile errors (module missing).

- [ ] **Step 3: Implement** the types and functions above. `AfterCompletion` with `Weeks`/`Months` ignores `on`; month addition clamps the day with `Month::length`. `next_occurrence` for `Calendar` iterates candidate dates from `after + 1` within on-cycle periods (week index / month index relative to `start`, `% n == 0`); it must terminate in at most one cycle's worth of steps. Keep each fn under CCN 10 by splitting per `Rule` variant.

- [ ] **Step 4: Run** `cargo +1.98 test -p anamnesis-core schedule` → PASS.

- [ ] **Step 5: Commit** (after `just quality`), including the spec file:

```bash
git add docs/superpowers/specs/2026-11-12-recurring-tasks-design.md docs/superpowers/plans/2026-11-13-recurring-tasks.md crates/anamnesis-core/src/schedule crates/anamnesis-core/src/lib.rs
git commit -m "Add Schedule value type for recurring tasks"
```

---

### Task 2: Schedule grammar — `parse` and `describe`

**Files:**
- Create: `crates/anamnesis-core/src/schedule/grammar.rs`
- Modify: `crates/anamnesis-core/src/schedule/mod.rs` (`mod grammar; pub use grammar::{parse, describe, ParseError};`), `lib.rs` re-exports

**Interfaces:**
- Consumes: Task 1 types, `first_start`.
- Produces:
  - `pub fn parse(input: &str, today: Date) -> Result<Schedule, ParseError>` — `today` fills `Calendar { start: first_start(..) }` / `AfterCompletion { set_on: today }`.
  - `pub fn describe(schedule: &Schedule) -> String` — canonical phrase.
  - `pub struct ParseError { pub word: Option<String>, pub hint: String }` with `Display` = `didn't understand '<word>' — <hint>` (or just the hint if `word` is `None`).

Grammar is exactly the spec's §Grammar. Canonical forms `describe` emits: `every tue at 6pm until 8am`, `every 3 months after done`, `every week after done`, `every mon, thu`, `every 2 weeks on sat`, `every month on the last fri`, `every month on the 31st`, `every 10 days`, times as `6pm` / `6:30pm` / `noon` / `midnight`.

- [ ] **Step 1: Write failing tests**

```rust
const TODAY: Date = date!(2026-11-13); // a Friday
#[rstest]
#[case("every tue at 6pm until 8am")]
#[case("every 3 months after done")]
#[case("every week after done")]
#[case("every mon, thu")]
#[case("every 2 weeks on sat")]
#[case("every month on the last fri")]
#[case("every month on the 31st")]
#[case("every 10 days")]
#[case("every month on the 2nd sat at 9:30am")]
fn canonical_phrases_round_trip(#[case] p: &str) {
    assert_eq!(describe(&parse(p, TODAY).unwrap()), p);
}
#[rstest]
#[case("Every Tuesday at 18:00 until 08:00", "every tue at 6pm until 8am")]
#[case("3 months after done", "every 3 months after done")]
#[case("every weekday", "every mon, tue, wed, thu, fri")]
#[case("every weekend at noon", "every sat, sun at noon")]
#[case("  every   2 weeks on Saturdays ", "every 2 weeks on sat")]
#[case("every month on the last day", "every month on the last day")]
fn variants_normalise(#[case] input: &str, #[case] canonical: &str) {
    assert_eq!(describe(&parse(input, TODAY).unwrap()), canonical);
}
#[test]
fn trash_parses_to_deadline_schedule() {
    let s = parse("every tue at 6pm until 8am", TODAY).unwrap();
    assert!(s.is_deadline());
    assert_eq!(s.pop_in, Some(PopIn { at: time!(18:00), until: Some(time!(8:00)) }));
    assert_eq!(s.anchor, Anchor::Calendar { start: date!(2026-11-17) });
}
#[rstest]
#[case("", None)]
#[case("every fortnight", Some("fortnight"))]
#[case("every 0 days", Some("0"))]
#[case("every month on the 32nd", Some("32nd"))]
#[case("every tue at 25:00", Some("25:00"))]
#[case("every 2 weeks on", None)]
#[case("every month on the 5th fri", Some("5th"))]
#[case("every tue until 8am", Some("until"))] // until requires at
fn bad_input_is_an_error_not_a_panic(#[case] input: &str, #[case] word: Option<&str>) {
    let err = parse(input, TODAY).unwrap_err();
    assert_eq!(err.word.as_deref(), word);
    assert!(!err.hint.is_empty());
}
#[test]
fn fortnight_hint_suggests_every_2_weeks() {
    assert!(parse("every fortnight", TODAY).unwrap_err().hint.contains("every 2 weeks"));
}
```

- [ ] **Step 2: Run** `cargo +1.98 test -p anamnesis-core grammar` → FAIL (missing).
- [ ] **Step 3: Implement** a hand-written tokenizer (lowercase, split on whitespace and commas) plus recursive-descent functions per grammar production (`cadence`, `on_spec`, `time`, `weekday_list`). Call `Schedule::validate` before returning. Keep each production its own fn.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** — `just quality`, then `git commit -m "Parse and describe recurring-task schedules"`.

---

### Task 3: Per-task recurrence state and pure lifecycle

**Files:**
- Create: `crates/anamnesis-core/src/recurring.rs`
- Modify: `crates/anamnesis-core/src/task.rs` (`Task` gains `pub recurrence: Option<TaskRecurrence>`; `create_task` sets `None`), `lib.rs`; every `Task { .. }` literal in the workspace (fakes, tests) gets `recurrence: None`.

**Interfaces:**
- Consumes: `Schedule`, `Placement`, `Task`.
- Produces:
  - `pub struct TaskRecurrence { pub schedule: Schedule, pub next_due_at: Option<Timestamp>, pub expires_at: Option<Timestamp>, pub last_completed_at: Option<Timestamp>, pub due_since: Option<Timestamp> }` — `next_due_at == None` means paused (explicitly archived); the schedule is kept so restoring can resume it.
  - `pub fn pause_recurrence(task: &Task) -> Task` — `next_due_at`, `expires_at`, `due_since` → `None`; schedule kept.
  - `pub struct Firing { pub next_due_at: Timestamp, pub expires_at: Option<Timestamp> }` — the caller-computed timing for the occurrence being fired.
  - `pub fn set_schedule(task: &Task, schedule: Schedule, next_due_at: Timestamp) -> Task` — replaces recurrence, keeps `last_completed_at` if one existed, clears `due_since`/`expires_at`; placement untouched.
  - `pub fn clear_schedule(task: &Task) -> Task`
  - `pub fn record_completion(task: &Task, now: Timestamp, next_due_at: Option<Timestamp>) -> Task` — stamps `last_completed_at`, clears `due_since`/`expires_at`; replaces `next_due_at` only when `Some` (after-completion schedules).
  - `pub fn fire(task: &Task, in_done_column: bool, entry_end: Placement, firing: Firing, now: Timestamp) -> Task` — rules below.
  - `pub fn expire(task: &Task, in_done_column: bool, now: Timestamp) -> Task`
  - `pub fn reset_checklist_item(item: &Task, in_done_column: bool) -> Option<Task>` — `Some` (un-archived, `Below`) only if item is archived or in a done column.

`fire` rules (spec §Lifecycle 3), all without touching bounce/offer/touch fields:
- Archived → un-archive (`archived_at = None`).
- If on board and **not** in a done column → placement unchanged.
- Else deadline schedule → `entry_end` unless `firing.expires_at <= now` (stale window: leave placement as-is, which is `Below` or done/archived-and-now-unarchived → set `Below`); `expires_at = firing.expires_at`.
- Else interval schedule → `Below`; `due_since` = existing `due_since` or `now`.
- Always `next_due_at = Some(firing.next_due_at)`.

`expire`: if `expires_at <= now` and on board and not in a done column → `Below`; always clear `expires_at`. Otherwise just clear `expires_at`.

- [ ] **Step 1: Write failing tests** in `recurring.rs`:

```rust
#[test] fn fire_deadline_from_done_lands_at_entry_end_with_expiry() { /* placement==entry_end, expires_at==Some(F.expires), next_due_at==F.next, bounce_count unchanged */ }
#[test] fn fire_deadline_from_archive_unarchives_and_raises() {}
#[test] fn fire_deadline_with_stale_window_goes_below_not_board() { /* firing.expires_at < now → Below */ }
#[test] fn fire_leaves_task_already_up_and_unfinished_where_it_is() {}
#[test] fn fire_interval_goes_below_and_sets_due_since_once() { /* fire twice: due_since keeps the first now */ }
#[test] fn fire_never_touches_bounce_offer_or_touch_fields() {}
#[test] fn expire_lowers_without_bounce() { /* bounce_count, last_bounced_at unchanged */ }
#[test] fn expire_after_user_already_dropped_it_is_a_noop_on_placement() { /* Review Focus 4: Below stays Below, bounce_count from the user's drop preserved */ }
#[test] fn expire_leaves_a_finished_chore_in_done() {}
#[test] fn record_completion_clears_due_and_expiry_and_only_replaces_next_due_when_given() {}
#[test] fn set_schedule_keeps_last_completed_and_clears_due_since() {}
#[test] fn reset_checklist_item_only_touches_done_or_archived_items() {}
#[test] fn pause_keeps_schedule_and_clears_timing() {}
```

- [ ] **Step 2: Run** `cargo +1.98 test -p anamnesis-core recurring` → FAIL.
- [ ] **Step 3: Implement**; then `cargo +1.98 build --workspace --all-targets` and add `recurrence: None` to every `Task` literal the compiler flags.
- [ ] **Step 4: Run** `cargo +1.98 test -p anamnesis-core` → PASS.
- [ ] **Step 5: Commit** — `just quality`; `git commit -m "Track recurrence state on tasks with pure fire/expire/complete rules"`.

---

### Task 4: Suggestion engine — cooldown exemption and the due slot

**Files:**
- Modify: `crates/anamnesis-core/src/suggest.rs`

**Interfaces:**
- Produces:
  - `TaskSummary.due_since: Option<Timestamp>` (from `task.recurrence.and_then(|r| r.due_since)` in `from_task`).
  - `SuggestionSettings.due_slot_percent: u8`.
  - `SuggestionReason::Due`.

Behaviour:
- `is_eligible`: a candidate with `due_since.is_some()` skips `off_cooldown`; every other rule still applies. `diagnose` unchanged in order.
- In `suggest`, after building `pool`, if any eligible task has `due_since`: draw `rng.next_f64() < due_slot_percent as f64 / 100.0` using the same seeded `SplitMix64` the draw already uses (derive it once so seed stability holds). On success, pick one due task with weight `(now - due_since).seconds + 86_400`, emit it with `SuggestionReason::Due`, remove it from the pool, and fill the remaining `offer_size - 1` slots with the existing `draw_offer`.

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn due_chore_offered_despite_recent_offer() { /* last_offered_at = now-1s, cooldown 1 day, due_since Some → appears in Offer */ }
#[test] fn non_due_task_still_respects_cooldown() {}
#[test] fn due_slot_takes_first_slot_about_eighty_percent_of_seeds() {
    // 1 due chore + 20 ordinary eligible tasks, wip free 3, due_slot_percent 80, 2_000 seeds:
    // count of offers containing a Due item in 0.75..=0.85 fraction.
}
#[test] fn due_slot_percent_zero_never_and_hundred_always() {}
#[test] fn full_board_stays_silent_with_due_chores() { /* Outcome::Full */ }
#[test] fn due_slot_is_seed_stable() { /* same seed → identical Outcome */ }
```

- [ ] **Step 2: Run** `cargo +1.98 test -p anamnesis-core suggest` → FAIL.
- [ ] **Step 3: Implement**; fix every `SuggestionSettings { .. }` literal (default `due_slot_percent: 80` in `anamnesis-app/src/settings.rs` defaults as `DEFAULT_DUE_SLOT_PERCENT: u8 = 80`).
- [ ] **Step 4: Run** `cargo +1.98 test --workspace` → PASS (adapters may need Task 5's column before settings round-trips; if a settings contract test fails only on the missing column, proceed to Task 5 before committing and commit both together).
- [ ] **Step 5: Commit** — `just quality`; `git commit -m "Give due chores a cooldown-exempt first suggestion slot"`.

---

### Task 5: Storage — migration, task/settings persistence, recurrence queries

**Files:**
- Create: `crates/anamnesis-adapters/migrations/{sqlite,postgres}/0008_recurring_tasks.sql`
- Modify: `crates/anamnesis-adapters/src/sql/task.rs` (`TASK_COLUMNS`, row decode, insert/update binds), `sql/settings.rs`, `sql/board_query.rs` (`suggestion_candidates` selects `due_since`), `crates/anamnesis-app/src/ports/repository.rs`, `crates/anamnesis-app/tests/domain_fakes/mod.rs` (in-memory impls)
- Test: `crates/anamnesis-adapters/tests/sql_store_contract.rs`

Migration (SQLite shown; Postgres uses `TIMESTAMPTZ`/`TEXT` matching existing timestamp columns in `0001_init.sql`):

```sql
ALTER TABLE tasks ADD COLUMN schedule_json TEXT;
ALTER TABLE tasks ADD COLUMN next_due_at <existing timestamp type>;
ALTER TABLE tasks ADD COLUMN expires_at <existing timestamp type>;
ALTER TABLE tasks ADD COLUMN last_completed_at <existing timestamp type>;
ALTER TABLE tasks ADD COLUMN due_since <existing timestamp type>;
CREATE INDEX tasks_next_due_at ON tasks (next_due_at) WHERE schedule_json IS NOT NULL;
CREATE INDEX tasks_expires_at ON tasks (expires_at) WHERE expires_at IS NOT NULL;
ALTER TABLE settings ADD COLUMN suggestion_due_slot_percent INTEGER NOT NULL DEFAULT 80;
```

**Interfaces:**
- Produces on `TaskRepository`:
  - `async fn list_recurrence_actionable(&self, now: Timestamp) -> Result<Vec<Task>, RepoError>` — `schedule_json IS NOT NULL AND (next_due_at <= now OR expires_at <= now)`, archived included (paused tasks have NULL `next_due_at`, so they never match).
  - `async fn earliest_recurrence_event(&self) -> Result<Option<Timestamp>, RepoError>` — `MIN` over `next_due_at` (scheduled) and `expires_at`.
  - `async fn list_checklist_including_archived(&self, parent_id: TaskId) -> Result<Vec<Task>, RepoError>`
- `TaskRecurrence` persists as: `schedule_json` = `serde_json` of `Schedule` (`NULL` ⇔ `recurrence == None`), other four fields one column each, all nullable.

- [ ] **Step 1: Write failing contract tests** (run against SQLite by default; Postgres via the existing `#[ignore]` path):

```rust
#[tokio::test] async fn recurring_task_round_trips() { /* insert task with Some(TaskRecurrence{schedule: parse("every tue at 6pm until 8am")…}); load == inserted */ }
#[tokio::test] async fn non_recurring_task_round_trips_as_none() {}
#[tokio::test] async fn actionable_includes_due_and_expired_and_archived_but_not_future() {}
#[tokio::test] async fn earliest_event_is_min_of_due_and_expiry() {}
#[tokio::test] async fn checklist_listing_includes_archived_items() {}
#[tokio::test] async fn settings_round_trip_due_slot_percent() {}
#[tokio::test] async fn suggestion_candidates_carry_due_since() {}
```

- [ ] **Step 2: Run** `cargo +1.98 test -p anamnesis-adapters --test sql_store_contract` → FAIL.
- [ ] **Step 3: Implement** migrations, encode/decode (`serde_json` is already an adapters dependency — confirm with `grep serde_json crates/anamnesis-adapters/Cargo.toml`; if absent, it is in the lockfile via other crates, add with the locked version), port methods, fakes. Settings form/handler get the field in Task 8.
- [ ] **Step 4: Run** `cargo +1.98 test --workspace` → PASS. Also `cargo +1.98 test -p anamnesis-adapters --test sql_store_contract -- --ignored` if `compose.postgres.yaml` is up; otherwise note it was not run.
- [ ] **Step 5: Commit** — `just quality`; `git commit -m "Persist task recurrence and the due-slot setting"`.

---

### Task 6: App use cases — set schedule, completion hook, fire/expire tick

**Files:**
- Create: `crates/anamnesis-app/src/use_cases/recurring.rs`
- Modify: `use_cases/mod.rs`, `lib.rs` exports, `use_cases/task.rs` (`raise_task`, `archive_task`, `unarchive_task`), `use_cases/board.rs` (`reposition_board_item`)
- Create: `crates/anamnesis-app/features/recurring_tasks.feature`, `crates/anamnesis-app/tests/steps/recurring_tasks.rs` (+ register in `steps/mod.rs`)

**Interfaces:**
- Consumes: Tasks 1–5.
- Produces:
  - `pub fn next_due_instant(schedule: &Schedule, after: Date, last_completed: Option<Date>, tz: &dyn TimezoneResolver, zone: &str) -> Result<Firing, AppError>` — date via `next_occurrence`; instant = `to_utc(date, pop_in.at)` or `to_utc(date, MIDNIGHT)`; `expires_at` = `to_utc(date or date+1 if until <= at, until)`. **Spring-forward gap:** if `to_utc` errs for a time that exists on neither side, retry with the time plus one hour (the first wall-clock time after the gap); a bad zone name still errors.
  - `pub async fn set_task_schedule(repo: &dyn TaskRepository, clock: &dyn Clock, tz: &dyn TimezoneResolver, zone: &str, role: Option<Role>, task_id: TaskId, schedule: Option<Schedule>) -> Result<Task, AppError>` — `Action::EditTask`; `None` clears.
  - `pub struct RecurrenceTick { pub fired: u32, pub expired: u32, pub skipped_inactive: u32 }`
  - `pub async fn run_recurrence_tick(deps: &RecurrenceDeps<'_>, now: Timestamp) -> Result<RecurrenceTick, AppError>` where `RecurrenceDeps { tasks, projects, board, tz, zone }` are the ports the tick genuinely needs together.

Behaviour:
- **Completion**: in `raise_task` and `reposition_board_item`, when the destination column `is_done` and the task was not already in it and `task.recurrence.is_some()`: `record_completion(task, now, next)` where `next` = `Some(next_due_instant(.., after: today, last_completed: today))` for `AfterCompletion`, `None` for `Calendar`. `raise_task` gains a `tz`/`zone` need — pass through as `RecurrenceDeps`-style args only if clippy/lizard allow cleanly; otherwise compute completion in a small `complete_if_recurring` helper called from both use cases after the move.
- **Explicit archive** (`archive_task`) calls `pause_recurrence`; **unarchive** (`unarchive_task`) of a paused recurring task calls `set_schedule` with its kept schedule and `next_due_instant(.., after: today ..)`. The sweep path (`archive_done_tasks`) does not pause. `archive_task`/`unarchive_task` gain the `tz`/`zone` they need for that.
- **Tick**: for each `list_recurrence_actionable(now)` task: skip (count `skipped_inactive`) if its project isn't `Active` — leave `next_due_at` as-is; if `expires_at <= now` → `expire`; if `next_due_at <= now` → compute `Firing` for the occurrence after **today's local date**, `fire` with `entry_end` = first column, position = its item count, `in_done_column` from `columns_with_items`; then reset its checklist via `list_checklist_including_archived` + `reset_checklist_item`. Bypass `check_wip_limit` by design (comment why, citing the spec).

- [ ] **Step 1: Write the failing feature** `recurring_tasks.feature` (scenarios, plain English, matching existing step style):

```gherkin
Scenario: Trash pops onto a full board at its time
  Given "To-Do" is a column with a work-in-progress limit of 1 that is not done
  And a task "Something else" on the board in "To-Do"
  And a task "Take out the trash" below the horizon in active project "Household"
  And "Take out the trash" repeats "every tue at 6pm until 8am"
  When it is Tuesday 2026-11-17 at 6:00pm and recurring tasks are checked
  Then task "Take out the trash" is on the board in "To-Do"
  And "To-Do" holds 2 items

Scenario: An unfinished trash chore quietly goes back down without a bounce
  ... When it is Wednesday 2026-11-18 at 8:00am and recurring tasks are checked
  Then task "Take out the trash" is below the horizon
  And task "Take out the trash" has bounced 0 times

Scenario: The furnace filter is due three months after it was last done
  Given "Replace furnace filter" repeats "3 months after done"
  When "Alice" raises "Replace furnace filter" into "Done" on 2026-11-30
  Then "Replace furnace filter" is next due on 2027-02-28

Scenario: Downtime across three Tuesdays fires once and skips stale windows
Scenario: A chore in a paused project does not fire, then fires once on reactivation
Scenario: A recurring task's finished checklist resets when it comes back
Scenario: Archiving a recurring task from its page stops it recurring
Scenario: Restoring it resumes the schedule from today
Scenario: The archive sweep does not stop a recurring task
Scenario: A 2:30am pop-in on spring-forward day fires at 3:30am   # Review Focus 1, zone America/New_York, 2027-03-14 (shifted forward by the gap length, per next_due_instant)
Scenario: Changing the schedule while it is due clears due-ness and recomputes next due   # Review Focus 3
```

Plus unit tests in `recurring.rs` for `next_due_instant` (deadline, interval midnight, `until` next-day rollover, gap retry, bad zone errors).

- [ ] **Step 2: Run** `cargo +1.98 test -p anamnesis-app --test cucumber` → new scenarios FAIL (undefined steps / assertions).
- [ ] **Step 3: Implement** use cases and step definitions (time control via the fake clock in `tests/support`).
- [ ] **Step 4: Run** `cargo +1.98 test -p anamnesis-app` → PASS.
- [ ] **Step 5: Commit** — `just quality`; `git commit -m "Fire, expire and complete recurring tasks in the app layer"`.

---

### Task 7: Recurrence ticker

**Files:**
- Create: `crates/anamnesis-web/src/recurrence_ticker.rs`
- Modify: `crates/anamnesis-web/src/lib.rs` (module), `main.rs` (spawn next to `sweep::spawn_ticker`, abort on shutdown like the others)

**Interfaces:**
- Consumes: `run_recurrence_tick`, `TaskRepository::earliest_recurrence_event`, `state.leases`.
- Produces: `pub fn spawn_ticker(state: AppState) -> tokio::task::JoinHandle<()>`; `pub fn sleep_for(now: Timestamp, earliest: Option<Timestamp>) -> Duration` (pure).

Constants: `RECURRENCE_JOB = "recurrence_tick"`, `MAX_SLEEP = 15 min`, `LEASE_TTL = 2 min`, `RETRY_INTERVAL = LEASE_TTL + 60 s`. Loop: check first, then sleep. A tick that failed or found the lease held sleeps `RETRY_INTERVAL` (same "never assume it wakes soon" rule as `sweep.rs`; copy that module doc's reasoning by reference, not text). Only `main.rs` calls `spawn_ticker`.

- [ ] **Step 1: Write failing unit tests** for `sleep_for`:

```rust
#[test] fn sleeps_until_earliest_event() { /* earliest = now+5min → 5min */ }
#[test] fn caps_at_fifteen_minutes() { /* now+3h → 15min; None → 15min */ }
#[test] fn past_event_means_check_now() { /* earliest < now → Duration::ZERO */ }
```

- [ ] **Step 2: Run** `cargo +1.98 test -p anamnesis-web recurrence_ticker` → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** → PASS; `cargo +1.98 build -p anamnesis-web`.
- [ ] **Step 5: Commit** — `just quality`; `git commit -m "Add the recurring-task ticker"`.

---

### Task 8: Web — Repeats editor (phrase + dropdowns), card glyph, suggestion copy, settings field

**Files:**
- Create: `crates/anamnesis-web/src/handlers/tasks/schedule.rs`, `crates/anamnesis-web/templates/_schedule_editor.html`
- Modify: `handlers/tasks/mod.rs`, `routes.rs` (`task_routes`), `templates/task.html` (include editor), `templates/_card.html` (glyph), board suggestion template/handler (`SuggestionReason::Due` → "due again"), `handlers/settings.rs` + `templates/settings.html` (`suggestion_due_slot_percent`, label "Chance a due chore gets the first suggestion (%)", 0–100)
- Test: `crates/anamnesis-web/tests/recurring_tasks.rs`, extend `tests/settings.rs`

**Interfaces:**
- Consumes: `parse`, `describe`, `upcoming`, `Schedule` types, `set_task_schedule`, `next_due_instant`.
- Produces routes:
  - `POST /tasks/{id}/schedule/preview` → `_schedule_editor.html` fragment (200, or 200 with inline error).
  - `POST /tasks/{id}/schedule` → save; redirect `303` to `/tasks/{id}` (HX: re-render fragment). Empty phrase with `source=phrase` clears.
- Form fields: `source` (`phrase`|`builder`), `phrase`, `every_n`, `unit` (`days`|`weeks`|`months`), `anchor` (`calendar`|`after_done`), `weekdays` (multi), `month_mode` (`day`|`last_day`|`nth`), `month_day`, `nth`, `nth_weekday`, `pop_in` (checkbox), `pop_in_hour`/`pop_in_minute`/`pop_in_ampm`, `until` (checkbox), `until_hour`/`until_minute`/`until_ampm`.
- `fn schedule_from_builder(form: &ScheduleForm, today: Date) -> Result<Schedule, ParseError>` — split into `rule_from_builder` and `pop_in_from_builder` so each stays simple; errors reuse `ParseError` with `word: None`.
- No-JS: plain form submit to the save route; if `source=phrase`, the phrase wins.

Template renders: the canonical phrase in the text input; dropdowns set from the `Schedule`; rows shown/hidden by `unit`/`anchor` server-side (no client logic beyond htmx `hx-post` on `change`/`keyup changed delay:400ms` targeting the section); "Next:" with three occurrences formatted `Tue Nov 17 6:00pm` in local time; error text inline under the input. Card glyph: `↻` with `title="Repeats: <phrase>"`, no colour change.

- [ ] **Step 1: Write failing web tests**

```rust
#[tokio::test] async fn preview_from_phrase_shows_canonical_phrase_and_three_dates() { /* "Every Tuesday at 18:00 until 08:00" → body contains "every tue at 6pm until 8am" and "Tue Nov 17 6:00pm" */ }
#[tokio::test] async fn preview_from_builder_fills_phrase() { /* unit=months, month_mode=nth, nth=last, nth_weekday=fri → "every month on the last fri" */ }
#[tokio::test] async fn builder_and_phrase_agree_on_round_trip() { /* builder form for the parsed trash schedule re-renders identical selected options */ }
#[tokio::test] async fn bad_phrase_shows_inline_error_and_keeps_input() { /* "every fortnight" → 200, contains "didn't understand 'fortnight'" */ }
#[tokio::test] async fn save_sets_schedule_and_empty_phrase_clears_it() {}
#[tokio::test] async fn save_requires_edit_permission() { /* no-role user → 403 */ }
#[tokio::test] async fn recurring_card_shows_repeat_glyph() {}
#[tokio::test] async fn due_suggestion_reads_due_again() {}
// tests/settings.rs:
#[tokio::test] async fn due_slot_percent_is_editable_and_bounded() { /* 101 → 400 */ }
```

- [ ] **Step 2: Run** `cargo +1.98 test -p anamnesis-web --test recurring_tasks` → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo +1.98 test -p anamnesis-web` → PASS; then `just run` and click through: set "every tue at 6pm until 8am" via each editor, confirm preview and card glyph. Report what was seen.
- [ ] **Step 5: Commit** — `just quality`; `git commit -m "Add the Repeats editor, card glyph and due-chore suggestion copy"`.

---

### Task 9: Docs

**Files:**
- Modify: `docs/DOMAIN.md` (§3 Task fields; §5 eligibility + composition: cooldown exemption, due slot; §6 replace "recurring tasks are not in scope now" with a summary pointing to the spec; §12 add decisions: "time of day makes a deadline chore", "one task cycles", "calendar phase from first occurrence, not epoch", "explicit archive stops recurrence"), `README.md` (remove the "No recurring tasks" bullet; add a short "Recurring chores" paragraph under the feature overview).

- [ ] **Step 1: Edit docs.**
- [ ] **Step 2: Verify** `cargo +1.98 test --workspace` and `just check` → all PASS.
- [ ] **Step 3: Commit** — `git commit -m "Document recurring tasks"`.
