Feature: Linking tasks
  A Relationship is a standalone edge living outside any project
  (docs/DOMAIN.md SS3): "any task may relate to any task across areas and
  projects, because real blockers cross domains constantly." Only a
  built-in kind may cross projects; a project's own custom vocabulary only
  means anything within that one project, because there is no shared owner
  for it to belong to on the far side of a cross-project edge.

  Scenario: A built-in kind links two tasks in different projects
    Given a task "Pick tile" below the horizon in project "Kitchen Remodel"
    And a task "Order grout" below the horizon in project "Yard"
    And "Alice" is a Member of "Kitchen Remodel"
    When "Alice" links "Order grout" as blocking "Pick tile"
    Then "Pick tile" is blocked by "Order grout"

  Scenario: A project's custom kind links two tasks within that same project
    Given a task "Pick tile" below the horizon in project "Kitchen Remodel"
    And a task "Pick grout colour" below the horizon in project "Kitchen Remodel"
    And "Priya" is a Project Admin of "Kitchen Remodel"
    And "inspired by" is a custom relationship kind of project "Kitchen Remodel"
    When "Priya" links "Pick grout colour" to "Pick tile" using "inspired by"
    Then access is granted

  Scenario: A project's custom kind is refused across projects
    Given a task "Pick tile" below the horizon in project "Kitchen Remodel"
    And a task "Order grout" below the horizon in project "Yard"
    And "Priya" is a Project Admin of "Kitchen Remodel"
    And "inspired by" is a custom relationship kind of project "Kitchen Remodel"
    When "Priya" tries to link "Order grout" to "Pick tile" using "inspired by"
    Then the link is refused because a custom kind may only be used within its own project

  Scenario: A task cannot relate to itself
    Given a task "Pick tile" below the horizon in project "Kitchen Remodel"
    And "Alice" is a Member of "Kitchen Remodel"
    When "Alice" tries to link "Pick tile" as blocking "Pick tile"
    Then the link is refused because a task cannot relate to itself

  Scenario: Deleting a relationship breaks the link
    Given a task "Pick tile" below the horizon in project "Kitchen Remodel"
    And a task "Order grout" below the horizon in project "Yard"
    And "Alice" is a Member of "Kitchen Remodel"
    And "Order grout" is already blocking "Pick tile"
    When "Alice" deletes the link between "Order grout" and "Pick tile"
    Then "Pick tile" is not blocked by "Order grout"
