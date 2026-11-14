Feature: Recurring tasks
  Household chores come back on their own. A chore with a time of day is a
  deadline chore: it pops onto the board at that time, even over a full
  column, and quietly goes back down when its window closes. A chore
  without one is an interval chore: it comes due below the horizon for the
  suggestion engine to offer. One task cycles; nothing ever counts the
  clock's movements as the user's bounces (docs/superpowers/specs/
  2026-11-12-recurring-tasks-design.md, "Lifecycle").

  Background:
    Given the time zone is "America/New_York"
    And it is Monday 2026-11-16 at 9:00am
    And "To-Do" is a column with a work-in-progress limit of 1 that is not done
    And "Done" is a done column with no work-in-progress limit
    And "Alice" is a Member of "Household"

  Scenario: Trash pops onto a full board at its time
    Given a task "Something else" on the board in "To-Do"
    And a task "Take out the trash" below the horizon in active project "Household"
    And "Take out the trash" repeats "every tue at 6pm until 8am"
    When it is Tuesday 2026-11-17 at 5:59pm and recurring tasks are checked
    Then "Take out the trash" is below the horizon
    When it is Tuesday 2026-11-17 at 6:00pm and recurring tasks are checked
    Then task "Take out the trash" is on the board in "To-Do"
    And "To-Do" holds 2 items

  Scenario: An unfinished trash chore quietly goes back down without a bounce
    Given a task "Take out the trash" below the horizon in active project "Household"
    And "Take out the trash" repeats "every tue at 6pm until 8am"
    When it is Tuesday 2026-11-17 at 6:00pm and recurring tasks are checked
    Then task "Take out the trash" is on the board in "To-Do"
    When it is Wednesday 2026-11-18 at 8:00am and recurring tasks are checked
    Then "Take out the trash" is below the horizon
    And "Take out the trash" has bounced 0 times
    And "Take out the trash" is next due on 2026-11-24

  Scenario: An interval chore comes due below the horizon, never onto the board
    Given a task "Vacuum the stairs" below the horizon in active project "Household"
    And "Vacuum the stairs" repeats "every sat"
    When it is Saturday 2026-11-21 at 9:00am and recurring tasks are checked
    Then "Vacuum the stairs" is below the horizon
    And "Vacuum the stairs" is due
    And "Vacuum the stairs" is next due on 2026-11-28

  Scenario: The furnace filter is due three months after it was last done
    Given a task "Replace furnace filter" below the horizon in active project "Household"
    And "Replace furnace filter" repeats "every 3 months after done"
    When "Alice" raises "Replace furnace filter" into "Done" on 2026-11-30
    Then "Replace furnace filter" is next due on 2027-02-28

  Scenario: Dragging a chore into Done on the board also completes it
    Given a task "Replace furnace filter" on the board in "To-Do"
    And "Replace furnace filter" repeats "every 3 months after done"
    When "Alice" drags "Replace furnace filter" into "Done" on 2026-11-30
    Then "Replace furnace filter" is next due on 2027-02-28

  Scenario: An after-done chore left undone comes due once, not on every check
    Given a task "Replace furnace filter" below the horizon in active project "Household"
    And "Replace furnace filter" repeats "every week after done"
    When it is Tuesday 2026-11-17 at 9:00am and recurring tasks are checked
    Then 1 recurring task fired
    And "Replace furnace filter" is due
    When recurring tasks are checked again
    Then 0 recurring tasks fired

  Scenario: Downtime across three Tuesdays fires once and skips stale windows
    Given a task "Take out the trash" below the horizon in active project "Household"
    And "Take out the trash" repeats "every tue at 6pm until 8am"
    When it is Thursday 2026-12-03 at 12:00pm and recurring tasks are checked
    Then 1 recurring task fired
    And "Take out the trash" is below the horizon
    And "Take out the trash" is next due on 2026-12-08
    When recurring tasks are checked again
    Then 0 recurring tasks fired

  Scenario: Coming back up inside the latest window still puts the chore up
    Given a task "Take out the trash" below the horizon in active project "Household"
    And "Take out the trash" repeats "every tue at 6pm until 8am"
    When it is Tuesday 2026-12-01 at 9:00pm and recurring tasks are checked
    Then 1 recurring task fired
    And task "Take out the trash" is on the board in "To-Do"
    When it is Wednesday 2026-12-02 at 8:00am and recurring tasks are checked
    Then "Take out the trash" is below the horizon

  Scenario: A chore in a paused project does not fire, then fires once on reactivation
    Given a task "Vacuum the stairs" below the horizon in active project "Household"
    And "Vacuum the stairs" repeats "every sat"
    And project "Household" is put on hold
    When it is Saturday 2026-11-28 at 9:00am and recurring tasks are checked
    Then 0 recurring tasks fired
    And 1 recurring task was skipped as inactive
    And "Vacuum the stairs" is not due
    When project "Household" is reactivated
    And it is Monday 2026-12-07 at 9:00am and recurring tasks are checked
    Then 1 recurring task fired
    And "Vacuum the stairs" is due
    And "Vacuum the stairs" is next due on 2026-12-12

  Scenario: A recurring task's finished checklist resets when it comes back
    Given a task "Clean the bathroom" below the horizon in active project "Household"
    And a task "Scrub the tub" on the board in "Done"
    And a task "Mop the floor" that was archived
    And a task "Wipe the mirror" on the board in "To-Do"
    And "Scrub the tub" is a checklist item of "Clean the bathroom"
    And "Mop the floor" is a checklist item of "Clean the bathroom"
    And "Wipe the mirror" is a checklist item of "Clean the bathroom"
    And "Clean the bathroom" repeats "every sat"
    When it is Saturday 2026-11-21 at 9:00am and recurring tasks are checked
    Then "Scrub the tub" is below the horizon
    And task "Mop the floor" is not archived
    And "Mop the floor" is below the horizon
    And task "Wipe the mirror" is on the board in "To-Do"
    And "Scrub the tub" has bounced 0 times

  Scenario: Archiving a recurring task from its page stops it recurring
    Given a task "Take out the trash" below the horizon in active project "Household"
    And "Take out the trash" repeats "every tue at 6pm until 8am"
    When "Alice" archives task "Take out the trash"
    And it is Tuesday 2026-11-17 at 6:00pm and recurring tasks are checked
    Then task "Take out the trash" is archived
    And "Take out the trash" is not scheduled to come back
    And "Take out the trash" still repeats "every tue at 6pm until 8am"

  Scenario: Restoring it resumes the schedule from today
    Given a task "Take out the trash" below the horizon in active project "Household"
    And "Take out the trash" repeats "every tue at 6pm until 8am"
    When "Alice" archives task "Take out the trash"
    And it is Wednesday 2026-11-25 at 9:00am
    And "Alice" unarchives task "Take out the trash"
    Then "Take out the trash" is next due on 2026-12-01

  Scenario: The archive sweep does not stop a recurring task
    Given a task "Take out the trash" below the horizon in active project "Household"
    And "Take out the trash" repeats "every tue at 6pm until 8am"
    When it is Tuesday 2026-11-17 at 6:00pm and recurring tasks are checked
    And "Alice" raises "Take out the trash" into "Done"
    And "Alice" archives all done work
    Then task "Take out the trash" is archived
    When it is Tuesday 2026-11-24 at 6:00pm and recurring tasks are checked
    Then task "Take out the trash" is not archived
    And task "Take out the trash" is on the board in "To-Do"
    And "Done" holds 0 items

  Scenario: A 2:30am pop-in on spring-forward day fires at 3:30am
    Given it is Saturday 2027-03-13 at 9:00am
    And a task "Water the seedlings" below the horizon in active project "Household"
    And "Water the seedlings" repeats "every day at 2:30am"
    Then "Water the seedlings" is next due at 2027-03-14 3:30am
    When it is Sunday 2027-03-14 at 3:29am and recurring tasks are checked
    Then "Water the seedlings" is below the horizon
    When it is Sunday 2027-03-14 at 3:30am and recurring tasks are checked
    Then task "Water the seedlings" is on the board in "To-Do"
    And "Water the seedlings" is next due at 2027-03-15 2:30am

  Scenario: Changing the schedule while it is due clears due-ness and recomputes next due
    Given a task "Water the plants" below the horizon in active project "Household"
    And "Water the plants" repeats "every day"
    When it is Tuesday 2026-11-17 at 9:00am and recurring tasks are checked
    Then "Water the plants" is due
    When "Alice" changes the schedule of "Water the plants" to "every sat"
    Then "Water the plants" is not due
    And "Water the plants" is next due on 2026-11-21
    And "Water the plants" is below the horizon

  Scenario: Clearing the schedule stops a task repeating
    Given a task "Water the plants" below the horizon in active project "Household"
    And "Water the plants" repeats "every day"
    When "Alice" clears the schedule of "Water the plants"
    Then "Water the plants" does not repeat
