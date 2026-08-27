Feature: Raising and dropping tasks
  Raising a task onto the global task board, and dropping it back below the
  horizon, is how the horizon metaphor actually moves work. Dropping a task
  that never reached a "done" column counts as a bounce -- the system's only
  behavioural signal of real resistance to a task, tracked for softer prompt
  copy, never asked of the user directly (docs/DOMAIN.md SS1, SS5).

  Scenario: Dropping a raised task back below the horizon without finishing it counts as a bounce
    Given a task "Fix the fence" below the horizon in project "Yard"
    And "Doing" is a column with no work-in-progress limit that is not done
    And "Alice" is a Member of "Yard"
    When "Alice" raises "Fix the fence" into "Doing"
    Then "Fix the fence" is on the board
    When "Alice" drops "Fix the fence" back below the horizon without finishing it
    Then "Fix the fence" is below the horizon
    And "Fix the fence" has bounced 1 time

  Scenario: Dropping a task from a done column does not count as a bounce
    Given a task "Ship the report" below the horizon in project "Yard"
    And "Done" is a done column with no work-in-progress limit
    And "Alice" is a Member of "Yard"
    When "Alice" raises "Ship the report" into "Done"
    And "Alice" drops "Ship the report" back below the horizon, finished
    Then "Ship the report" has bounced 0 times

  Scenario: A column at its work-in-progress limit refuses a new arrival
    Given "To-Do" is a column with a work-in-progress limit of 1 that is not done
    And a task "Occupant" below the horizon in project "Yard"
    And a task "Newcomer" below the horizon in project "Yard"
    And "Alice" is a Member of "Yard"
    When "Alice" raises "Occupant" into "To-Do"
    Then "Occupant" is on the board
    When "Alice" raises "Newcomer" into "To-Do"
    Then the move is refused because the column is at its work-in-progress limit
