Feature: Access control
  docs/DOMAIN.md names three roles -- System Admin, Project Admin, Member --
  and states some of their responsibilities without spelling out every
  capability. This is the matrix `crate::policy` builds on top of those
  three predicates, exercised here through the real use cases (role check,
  port, core transition) rather than the predicates directly.

  Scenario: A user with no role at all cannot view a project or its tasks
    Given a task "Pick tile" below the horizon in project "Kitchen Remodel"
    When "Eve" (with no role) tries to view project "Kitchen Remodel"
    Then access is refused
    When "Eve" (with no role) tries to view task "Pick tile"
    Then access is refused

  Scenario: A member can do ordinary task work but cannot manage field definitions
    Given a task "Pick tile" below the horizon in project "Kitchen Remodel"
    And "Alice" is a Member of "Kitchen Remodel"
    When "Alice" tries to view project "Kitchen Remodel"
    Then access is granted
    When "Alice" tries to view task "Pick tile"
    Then access is granted
    When "Alice" tries to add a field definition to project "Kitchen Remodel"
    Then access is refused

  Scenario: A project admin can manage field definitions and archive the project
    Given a task "Pick tile" below the horizon in project "Kitchen Remodel"
    And "Priya" is a Project Admin of "Kitchen Remodel"
    When "Priya" tries to add a field definition to project "Kitchen Remodel"
    Then access is granted
    When "Priya" tries to archive project "Kitchen Remodel"
    Then access is granted

  Scenario: Only a System Admin may manage areas
    Given "Priya" is a Project Admin of "Kitchen Remodel"
    And "Sam" is a System Admin
    When "Priya" tries to create an area named "Garage"
    Then access is refused
    When "Sam" tries to create an area named "Garage"
    Then access is granted
