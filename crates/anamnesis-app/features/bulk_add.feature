Feature: Bulk-adding areas, projects, and tasks
  Everything used to go in one at a time, which is painful for initial setup
  or for transcribing a project plan already fully formed in someone's head
  (issue #34). `bulk_create_areas`/`bulk_create_projects`/`bulk_create_tasks`
  each run the same single-item use case once per title, so a batch obeys
  the exact same authorization and validation rules a lone creation would —
  these scenarios exercise that directly. (The web layer's own
  newline-per-title parsing and HTTP-level behaviour on top of these use
  cases is covered separately, by `anamnesis-web`'s `tests/bulk_add.rs`.)

  Scenario: Bulk-creating areas makes one per title
    Given "Priya" is a System Admin
    When "Priya" bulk-creates areas: "Home", "Health", "Finances"
    Then areas "Home", "Health", "Finances" all exist

  Scenario: Bulk-creating projects makes one per title, in the given area
    Given "Priya" is a Project Admin of area "Home Ops"
    When "Priya" bulk-creates projects in area "Home Ops": "Repaint the shed", "Clean the gutters"
    Then projects "Repaint the shed", "Clean the gutters" all exist in area "Home Ops"

  Scenario: Bulk-creating tasks makes one per title, in the given project
    Given "Priya" is a Project Admin of area "Home Ops"
    When "Priya" creates a project named "Repaint the shed" in area "Home Ops"
    And "Priya" bulk-creates tasks in project "Repaint the shed": "Buy primer", "Sand the trim"
    Then tasks "Buy primer", "Sand the trim" all exist in project "Repaint the shed"

  Scenario: A rejected title does not cost the rest of the batch
    Given "Priya" is a System Admin
    When "Priya" bulk-creates areas including one title over 200 characters, plus "Home" and "Health"
    Then areas "Home", "Health" all exist
    And the bulk create rejected exactly 1 title
