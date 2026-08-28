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

  # A gap Phase D surfaced: Areas are the top-level container, but roles
  # were only ever project-scoped, so anything area-level had nowhere to
  # hang except System Admin. The fix: Areas are a real membership scope,
  # and a Project inherits its Area's role when it has no explicit project
  # role of its own. A later correction: an Area grant and a Project grant
  # are independent and stack, by analogy to `chmod` -- the effective role
  # is the *strongest* of the two, never the more specific one, so adding a
  # grant can never subtract capability.

  Scenario: A role on the Area alone is enough to view it and start a project there
    Given "Priya" is a Project Admin of the area that contains "Kitchen Remodel"
    When "Priya" tries to view the area that contains "Kitchen Remodel"
    Then access is granted
    When "Priya" tries to create a project named "Pantry Redo" in the area that contains "Kitchen Remodel"
    Then access is granted

  Scenario: A role on one project neither leaks to its sibling nor to the Area
    Given "Bob" is a Member of "Kitchen Remodel"
    And "Garage Sale" is another project in the area that contains "Kitchen Remodel"
    When "Bob" tries to view project "Garage Sale"
    Then access is refused
    When "Bob" tries to view the area that contains "Kitchen Remodel"
    Then access is refused

  Scenario: A lesser explicit project role does not demote a stronger inherited Area role
    Given "Priya" is a Project Admin of the area that contains "Kitchen Remodel"
    And "Priya" is a Member of "Kitchen Remodel"
    When "Priya" tries to add a field definition to project "Kitchen Remodel"
    Then access is granted

  Scenario: A System Admin needs no Area or project membership row at all
    Given "Sam" is a System Admin
    When "Sam" tries to view the area that contains "Kitchen Remodel"
    Then access is granted
    When "Sam" tries to view project "Kitchen Remodel"
    Then access is granted

  # Gap 1: the write half of MembershipQuery. Granting a role carries a
  # hazard ordinary view/edit actions do not -- the content being written
  # can itself grant capability -- so these prove the two rules that close
  # it off: System Admin can only ever be granted through the dedicated
  # grant, never smuggled in through an Area/Project grant; and a grant is
  # always checked against the actor's role on the *specific* scope being
  # acted on, never a role held elsewhere.

  Scenario: A Project Admin cannot grant System Admin
    Given "Priya" is a Project Admin of "Kitchen Remodel"
    When "Priya" tries to grant System Admin to "Mallory"
    Then access is refused

  Scenario: A Project Admin cannot grant a role on an area they do not administer
    Given "Priya" is a Project Admin of "Kitchen Remodel"
    When "Priya" tries to grant a Member role to "Bob" on the area that contains "Garden Shed"
    Then access is refused

  Scenario: Granting an area role lets that user view the area and revoking removes it
    Given "Sam" is a System Admin
    When "Sam" tries to grant a Member role to "Bob" on the area that contains "Kitchen Remodel"
    Then access is granted
    When "Bob" (with no role) tries to view the area that contains "Kitchen Remodel"
    Then access is granted
    When "Sam" tries to revoke Bob's role on the area that contains "Kitchen Remodel"
    Then access is granted
    When "Bob" (with no role) tries to view the area that contains "Kitchen Remodel"
    Then access is refused

  Scenario: The last System Admin cannot revoke their own System Admin
    Given "Sam" is a System Admin
    When "Sam" tries to revoke System Admin from "Sam"
    Then the revocation is refused because Sam is the last System Admin
    When "Sam" tries to view project "Kitchen Remodel"
    Then access is granted
