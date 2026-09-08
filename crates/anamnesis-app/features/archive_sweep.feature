Feature: Sweeping done work off the board
  "Archive all" and the scheduled sweep are the same operation
  (docs/DOMAIN.md SS6): whatever triggers it, every task sitting in an
  `is_done` column is archived, and a task in any other column -- even one
  the user has already finished but not yet dragged to Done -- is left
  exactly where it is. The manual button must work even if the scheduled
  sweep never fires, so it is gated the same as any other ordinary task
  work: any assigned role, never none at all.

  Scenario: Archive all sweeps every task sitting in a done column, and nothing else
    Given "Done" is a done column with no work-in-progress limit
    And "Doing" is a column with no work-in-progress limit that is not done
    And a task "Grout the tile" below the horizon in project "Kitchen Remodel"
    And a task "Pick tile" below the horizon in project "Kitchen Remodel"
    And "Alice" is a Member of "Kitchen Remodel"
    When "Alice" raises "Grout the tile" into "Done"
    And "Alice" raises "Pick tile" into "Doing"
    And "Alice" archives all done work
    Then task "Grout the tile" is archived
    And task "Pick tile" is not archived

  Scenario: A user with no role cannot archive done work
    When "Eve" (with no role) archives all done work
    Then access is refused
