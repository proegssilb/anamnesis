//! Steps for `recurring_tasks.feature`: setting schedules, completing
//! chores, and the recurrence tick, exercised through the real
//! `set_task_schedule`/`raise_task`/`reposition_board_item`/
//! `run_recurrence_tick` use cases against `domain_fakes::Fakes`, with local
//! times read through a real tzdb (`support::TzResolver`).
//!
//! Borrowed by phrase from other step files: the column, membership,
//! "raises into", "archives/unarchives task", "archives all done work",
//! "is below the horizon", "has bounced", "is a checklist item of", and
//! "task is (not) archived" steps.

use cucumber::{given, then, when};

use anamnesis_app::{
    BoardItemKind, BoardQuery, Clock, RecurrenceDeps, TimezoneResolver, raise_task,
    reposition_board_item, run_recurrence_tick, set_task_schedule,
};
use anamnesis_core::policy::Role;
use anamnesis_core::{Placement, ProjectStatus, TaskRecurrence, Timestamp};
use time::{Date, Time, Weekday};

use super::AppWorld;
use crate::support::TzResolver;

/// The project every task these steps create without naming one lives in.
const HOUSEHOLD: &str = "Household";

/// "2026-11-17".
fn parse_date(text: &str) -> Date {
    let parts: Vec<&str> = text.split('-').collect();
    let [year, month, day] = parts[..] else {
        panic!("bad scenario date {text:?}");
    };
    let month: u8 = month.parse().unwrap();
    Date::from_calendar_date(
        year.parse().unwrap(),
        month.try_into().unwrap(),
        day.parse().unwrap(),
    )
    .unwrap_or_else(|e| panic!("bad scenario date {text:?}: {e}"))
}

/// "6:00pm", "12:00pm", "2:30am".
fn parse_clock(text: &str) -> Time {
    let (digits, pm) = match text.strip_suffix("pm") {
        Some(rest) => (rest, true),
        None => (text.strip_suffix("am").expect("time ends in am/pm"), false),
    };
    let (hour, minute) = digits.split_once(':').expect("time is H:MM");
    let hour: u8 = hour.parse().unwrap();
    let hour = (hour % 12) + if pm { 12 } else { 0 };
    Time::from_hms(hour, minute.parse().unwrap(), 0).unwrap()
}

fn set_clock(world: &mut AppWorld, weekday: &str, date: &str, clock: &str) {
    let date = parse_date(date);
    let named: Weekday = weekday_named(weekday);
    assert_eq!(date.weekday(), named, "scenario says {date} is a {weekday}");
    world.set_local_time(date, parse_clock(clock));
}

fn weekday_named(name: &str) -> Weekday {
    [
        Weekday::Monday,
        Weekday::Tuesday,
        Weekday::Wednesday,
        Weekday::Thursday,
        Weekday::Friday,
        Weekday::Saturday,
        Weekday::Sunday,
    ]
    .into_iter()
    .find(|w| w.to_string() == name)
    .unwrap_or_else(|| panic!("unknown weekday {name:?}"))
}

fn local_date(world: &AppWorld, instant: Timestamp) -> Date {
    TzResolver.local_date(world.zone(), instant).unwrap()
}

fn recurrence(world: &AppWorld, task_name: &str) -> TaskRecurrence {
    world
        .domain_task_state(task_name)
        .recurrence
        .unwrap_or_else(|| panic!("{task_name:?} does not repeat"))
}

async fn check_recurring_tasks(world: &mut AppWorld) {
    let zone = world.zone().to_string();
    let deps = RecurrenceDeps {
        tasks: &world.domain,
        projects: &world.domain,
        board: &world.domain,
        search: &world.domain,
        tz: &TzResolver,
        zone: &zone,
    };
    let tick = run_recurrence_tick(&deps, world.clock.now())
        .await
        .expect("the recurrence tick must succeed");
    world.last_tick = Some(tick);
}

async fn change_schedule(world: &mut AppWorld, user: &str, task_name: &str, phrase: Option<&str>) {
    let role = world.domain_role(user);
    let today = local_date(world, world.clock.now());
    let schedule = phrase.map(|p| anamnesis_core::parse(p, today).expect("phrase must parse"));
    let task_id = world.domain_task_id(task_name);
    let result = set_task_schedule(
        &world.domain,
        &world.clock,
        &TzResolver,
        world.zone(),
        role,
        task_id,
        schedule,
    )
    .await;
    world.last_domain_error = result.err();
}

fn place(world: &mut AppWorld, task_name: &str, placement: Placement) {
    let mut task = world.domain_task_state(task_name);
    task.placement = placement;
    world.domain.seed_task(task);
}

fn set_project_status(world: &mut AppWorld, project_name: &str, status: ProjectStatus) {
    let id = world.domain_project(project_name);
    let mut project = world.domain.project(id);
    project.status = status;
    world.domain.seed_project(project);
}

// --- Given ---

#[given(regex = r#"^the time zone is "([^"]+)"$"#)]
async fn the_time_zone_is(world: &mut AppWorld, zone: String) {
    world.zone = Some(zone);
}

#[given(regex = r"^it is (\w+) (\d{4}-\d{2}-\d{2}) at (\d{1,2}:\d{2}[ap]m)$")]
async fn it_is(world: &mut AppWorld, weekday: String, date: String, clock: String) {
    set_clock(world, &weekday, &date, &clock);
}

#[given(regex = r#"^a task "([^"]+)" below the horizon in active project "([^"]+)"$"#)]
async fn a_task_in_active_project(world: &mut AppWorld, task_name: String, project: String) {
    world.domain_task(&task_name, &project);
    set_project_status(world, &project, ProjectStatus::Active);
}

#[given(regex = r#"^a task "([^"]+)" on the board in "([^"]+)"$"#)]
async fn a_task_on_the_board_in(world: &mut AppWorld, task_name: String, column_name: String) {
    world.domain_task(&task_name, HOUSEHOLD);
    let column = world.domain_column_id(&column_name);
    let position = world.domain.count_on_column(column).await.unwrap();
    place(world, &task_name, Placement::OnBoard { column, position });
}

#[given(regex = r#"^a task "([^"]+)" that was archived$"#)]
async fn a_task_that_was_archived(world: &mut AppWorld, task_name: String) {
    world.domain_task(&task_name, HOUSEHOLD);
    let task = world.domain_task_state(&task_name);
    let archived = anamnesis_core::archive_task(&task, world.clock.now()).unwrap();
    world.domain.seed_task(archived);
}

#[given(regex = r#"^"([^"]+)" repeats "([^"]+)"$"#)]
async fn repeats(world: &mut AppWorld, task_name: String, phrase: String) {
    world.set_domain_role("the scenario", Some(Role::SystemAdmin));
    change_schedule(world, "the scenario", &task_name, Some(&phrase)).await;
    assert_eq!(world.last_domain_error, None, "setting the schedule failed");
}

#[given(regex = r#"^project "([^"]+)" is put on hold$"#)]
async fn project_is_put_on_hold(world: &mut AppWorld, project: String) {
    set_project_status(world, &project, ProjectStatus::Pending);
}

// --- When ---

#[when(regex = r"^it is (\w+) (\d{4}-\d{2}-\d{2}) at (\d{1,2}:\d{2}[ap]m)$")]
async fn when_it_is(world: &mut AppWorld, weekday: String, date: String, clock: String) {
    set_clock(world, &weekday, &date, &clock);
}

#[when(
    regex = r"^it is (\w+) (\d{4}-\d{2}-\d{2}) at (\d{1,2}:\d{2}[ap]m) and recurring tasks are checked$"
)]
async fn it_is_and_checked(world: &mut AppWorld, weekday: String, date: String, clock: String) {
    set_clock(world, &weekday, &date, &clock);
    check_recurring_tasks(world).await;
}

#[when("recurring tasks are checked again")]
async fn checked_again(world: &mut AppWorld) {
    check_recurring_tasks(world).await;
}

#[when(regex = r#"^project "([^"]+)" is reactivated$"#)]
async fn project_is_reactivated(world: &mut AppWorld, project: String) {
    set_project_status(world, &project, ProjectStatus::Active);
}

#[when(regex = r#"^"([^"]+)" raises "([^"]+)" into "([^"]+)" on (\d{4}-\d{2}-\d{2})$"#)]
async fn raises_into_on(world: &mut AppWorld, user: String, task: String, col: String, on: String) {
    world.set_local_time(parse_date(&on), Time::from_hms(12, 0, 0).unwrap());
    let role = world.domain_role(&user);
    let task_id = world.domain_task_id(&task);
    let column = world.domain_column_id(&col);
    let result = raise_task(
        &world.domain,
        &world.domain,
        &world.clock,
        &TzResolver,
        world.zone(),
        role,
        task_id,
        column,
        0,
    )
    .await;
    world.last_domain_error = result.err();
}

#[when(regex = r#"^"([^"]+)" drags "([^"]+)" into "([^"]+)" on (\d{4}-\d{2}-\d{2})$"#)]
async fn drags_into_on(world: &mut AppWorld, user: String, task: String, col: String, on: String) {
    world.set_local_time(parse_date(&on), Time::from_hms(12, 0, 0).unwrap());
    let role = world.domain_role(&user);
    let task_id = world.domain_task_id(&task);
    let column = world.domain_column_id(&col);
    let result = reposition_board_item(
        &world.domain,
        &world.domain,
        &world.domain,
        &world.clock,
        &TzResolver,
        world.zone(),
        role,
        BoardItemKind::Task(task_id),
        column,
        0,
    )
    .await;
    world.last_domain_error = result.err();
}

#[when(regex = r#"^"([^"]+)" changes the schedule of "([^"]+)" to "([^"]+)"$"#)]
async fn changes_the_schedule(world: &mut AppWorld, user: String, task: String, phrase: String) {
    change_schedule(world, &user, &task, Some(&phrase)).await;
}

#[when(regex = r#"^"([^"]+)" clears the schedule of "([^"]+)"$"#)]
async fn clears_the_schedule(world: &mut AppWorld, user: String, task: String) {
    change_schedule(world, &user, &task, None).await;
}

// --- Then ---

#[then(regex = r#"^task "([^"]+)" is on the board in "([^"]+)"$"#)]
async fn is_on_the_board_in(world: &mut AppWorld, task_name: String, column_name: String) {
    let column = world.domain_column_id(&column_name);
    let task = world.domain_task_state(&task_name);
    assert!(
        matches!(task.placement, Placement::OnBoard { column: c, .. } if c == column),
        "expected {task_name:?} in {column_name:?}, got {:?}",
        task.placement
    );
}

#[then(regex = r#"^"([^"]+)" holds (\d+) items?$"#)]
async fn column_holds(world: &mut AppWorld, column_name: String, count: u32) {
    let column = world.domain_column_id(&column_name);
    assert_eq!(world.domain.count_on_column(column).await.unwrap(), count);
}

#[then(regex = r#"^"([^"]+)" is next due on (\d{4}-\d{2}-\d{2})$"#)]
async fn is_next_due_on(world: &mut AppWorld, task_name: String, on: String) {
    let next = recurrence(world, &task_name)
        .next_due_at
        .expect("not paused");
    assert_eq!(local_date(world, next), parse_date(&on));
}

#[then(regex = r#"^"([^"]+)" is next due at (\d{4}-\d{2}-\d{2}) (\d{1,2}:\d{2}[ap]m)$"#)]
async fn is_next_due_at(world: &mut AppWorld, task_name: String, on: String, clock: String) {
    let next = recurrence(world, &task_name)
        .next_due_at
        .expect("not paused");
    let shown = (
        local_date(world, next),
        TzResolver.local_time(world.zone(), next).unwrap(),
    );
    assert_eq!(shown, (parse_date(&on), parse_clock(&clock)));
}

#[then(regex = r"^(\d+) recurring tasks? fired$")]
async fn n_fired(world: &mut AppWorld, count: u32) {
    assert_eq!(world.last_tick.expect("no tick ran").fired, count);
}

#[then(regex = r"^(\d+) recurring tasks? (?:was|were) skipped as inactive$")]
async fn n_skipped(world: &mut AppWorld, count: u32) {
    assert_eq!(
        world.last_tick.expect("no tick ran").skipped_inactive,
        count
    );
}

#[then(regex = r#"^"([^"]+)" is due$"#)]
async fn is_due(world: &mut AppWorld, task_name: String) {
    assert!(recurrence(world, &task_name).due_since.is_some());
}

#[then(regex = r#"^"([^"]+)" is not due$"#)]
async fn is_not_due(world: &mut AppWorld, task_name: String) {
    assert_eq!(recurrence(world, &task_name).due_since, None);
}

#[then(regex = r#"^"([^"]+)" is not scheduled to come back$"#)]
async fn is_paused(world: &mut AppWorld, task_name: String) {
    assert_eq!(recurrence(world, &task_name).next_due_at, None);
}

#[then(regex = r#"^"([^"]+)" still repeats "([^"]+)"$"#)]
async fn still_repeats(world: &mut AppWorld, task_name: String, phrase: String) {
    let schedule = recurrence(world, &task_name).schedule;
    assert_eq!(anamnesis_core::describe(&schedule), phrase);
}

#[then(regex = r#"^"([^"]+)" does not repeat$"#)]
async fn does_not_repeat(world: &mut AppWorld, task_name: String) {
    assert_eq!(world.domain_task_state(&task_name).recurrence, None);
}
