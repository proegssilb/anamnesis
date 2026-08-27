Feature: Suggestions
  The suggestion engine raises work above the horizon when there is room. It
  goes quiet the moment the board is already at its work-in-progress limit
  -- a full board means the user is already carrying what they agreed to
  carry, and the system does not nag them about it -- but when there is
  room and genuinely nothing to offer, it explains itself plainly rather
  than leaving an unexplained empty slot.

  Scenario: The system stays silent when the board is already full
    Given a board with a work-in-progress limit of 3, currently holding 3 tasks
    And a task "Do the thing" below the horizon in an active project
    When a suggestion is requested
    Then the system offers nothing at all

  Scenario: The engine explains itself when there is room but no project is active
    Given a board with a work-in-progress limit of 3, currently holding 0 tasks
    And a task "Do the thing" below the horizon in a pending project
    When a suggestion is requested
    Then the system explains that no project is active

  Scenario: The engine explains itself when there is room but the backlog is empty
    Given a board with a work-in-progress limit of 3, currently holding 0 tasks
    When a suggestion is requested
    Then the system explains that the backlog is empty

  Scenario: A knotted pair is offered as a tangle instead of individually
    Given a board with a work-in-progress limit of 3, currently holding 0 tasks
    And a task "A" below the horizon in an active project
    And a task "B" below the horizon in an active project
    And "A" blocks "B"
    And "B" blocks "A"
    And tangles have been detected and stored
    When a suggestion is requested
    Then the system offers the tangle containing "A" and "B"
    And neither "A" nor "B" is offered on its own
