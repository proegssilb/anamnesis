Feature: A project's lifecycle
  A project starts Pending and only becomes Active while the system is
  under its active-project limit -- `count(status == Active) <=
  settings.active_project_limit` (docs/DOMAIN.md SS3, SS7). Creating a
  project is structural, not ordinary task work, so it is gated like
  editing one: Project Admin (or System Admin) in the Area it will live in,
  never a plain Member.

  Scenario: A newly created project starts Pending, not Active
    Given "Priya" is a Project Admin of area "Home"
    When "Priya" creates a project named "Kitchen Remodel" in area "Home"
    Then project "Kitchen Remodel" is Pending

  Scenario: Activating a project is refused once the system is already at its active-project limit
    Given "Priya" is a Project Admin of the area that contains "Kitchen Remodel"
    And "Garage Sale" is another project in the area that contains "Kitchen Remodel"
    When "Priya" tries to activate project "Garage Sale" against an active-project limit of 1
    Then the activation is refused because the active-project limit is reached

  Scenario: A member cannot activate a project even though they can view it
    Given a task "Pick tile" below the horizon in project "Kitchen Remodel"
    And "Alice" is a Member of "Kitchen Remodel"
    When "Alice" tries to activate project "Kitchen Remodel" against an active-project limit of 10
    Then access is refused

  Scenario: Archiving a project removes it from view; unarchiving restores it
    Given "Priya" is a Project Admin of "Kitchen Remodel"
    When "Priya" tries to archive project "Kitchen Remodel"
    Then access is granted
    And project "Kitchen Remodel" is archived
    When "Priya" tries to unarchive project "Kitchen Remodel"
    Then access is granted
    And project "Kitchen Remodel" is not archived
