Feature: Capturing and shaping a task
  Creating a task, editing it, archiving it, and using checklists to break
  it into pieces -- the ordinary lifecycle of a single unit of work
  (docs/DOMAIN.md SS2, SS3, SS4, SS7). A task starts below the horizon,
  costing nothing, because capture must stay near-zero friction. Checklist
  containment stays acyclic even though the sibling Relationship graph is
  deliberately allowed to tangle -- and an edit that lands on a stale copy
  is refused rather than silently overwriting someone else's work.

  Scenario: A newly captured task starts below the horizon
    Given "Alice" is a Member of "Kitchen Remodel"
    When "Alice" captures a task "Regrout the shower" in project "Kitchen Remodel"
    Then "Regrout the shower" is below the horizon

  Scenario: Archiving a task removes it from view; unarchiving restores it
    Given a task "Order grout" below the horizon in project "Kitchen Remodel"
    And "Alice" is a Member of "Kitchen Remodel"
    When "Alice" archives task "Order grout"
    Then task "Order grout" is archived
    When "Alice" unarchives task "Order grout"
    Then task "Order grout" is not archived

  Scenario: A checklist item can be raised independently of its parent
    Given a task "Regrout the shower" below the horizon in project "Kitchen Remodel"
    And a task "Buy tile spacers" below the horizon in project "Kitchen Remodel"
    And "Doing" is a column with no work-in-progress limit that is not done
    And "Alice" is a Member of "Kitchen Remodel"
    When "Alice" makes "Buy tile spacers" a checklist item of "Regrout the shower"
    Then "Buy tile spacers" is contained in "Regrout the shower"
    When "Alice" raises "Buy tile spacers" into "Doing"
    Then "Buy tile spacers" is on the board
    And "Regrout the shower" is below the horizon

  Scenario: A task cannot be made its own ancestor
    Given a task "Regrout the shower" below the horizon in project "Kitchen Remodel"
    And a task "Buy tile spacers" below the horizon in project "Kitchen Remodel"
    And "Alice" is a Member of "Kitchen Remodel"
    And "Buy tile spacers" is a checklist item of "Regrout the shower"
    When "Alice" tries to make "Regrout the shower" a checklist item of "Buy tile spacers"
    Then the reparenting is refused because it would contain its own ancestor

  Scenario: Saving a stale edit is refused so a second writer never silently clobbers the first
    Given a task "Regrout the shower" below the horizon in project "Kitchen Remodel"
    And "Alice" is a Member of "Kitchen Remodel"
    And "Bob" is a Member of "Kitchen Remodel"
    And "Alice" has task "Regrout the shower" open for editing
    When "Bob" edits task "Regrout the shower" to have title "Regrout the shower (redone)"
    Then "Alice" saving her stale edit of "Regrout the shower" is refused because it was concurrently modified
