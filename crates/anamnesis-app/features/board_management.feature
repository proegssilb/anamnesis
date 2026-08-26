Feature: Board management
  As the owner of a board, I can create it, organise it into columns, put
  cards in those columns, rename a card, and clean up when I am done with a
  card or the whole board.

  Scenario: Creating a board
    Given Alice is signed in
    When Alice creates a board named "Personal Projects"
    Then the board "Personal Projects" exists
    And Alice can see "Personal Projects" in their list of boards

  Scenario: Adding columns to a board
    Given Alice is signed in
    And Alice has a board named "Personal Projects"
    When Alice adds a column named "To Do" to "Personal Projects"
    And Alice adds a column named "Doing" to "Personal Projects"
    And Alice adds a column named "Done" to "Personal Projects"
    Then "Personal Projects" has the following columns in order:
      | title |
      | To Do |
      | Doing |
      | Done  |

  Scenario: Adding a card to a column
    Given Alice is signed in
    And Alice has a board named "Personal Projects" with a column named "To Do"
    When Alice adds a card "Write the report" to "To Do" on "Personal Projects"
    Then "To Do" on "Personal Projects" contains a card "Write the report"

  Scenario: Renaming a card
    Given Alice is signed in
    And Alice has a board named "Personal Projects" with a column named "To Do"
    And Alice adds a card "Write the report" to "To Do" on "Personal Projects"
    When Alice renames the card "Write the report" on "Personal Projects" to "Write the quarterly report"
    Then "To Do" on "Personal Projects" contains a card "Write the quarterly report"

  Scenario: Deleting a card
    Given Alice is signed in
    And Alice has a board named "Personal Projects" with a column named "To Do"
    And Alice adds a card "Write the report" to "To Do" on "Personal Projects"
    When Alice deletes the card "Write the report" from "Personal Projects"
    Then "To Do" on "Personal Projects" has no cards

  Scenario: Deleting a board
    Given Alice is signed in
    And Alice has a board named "Personal Projects"
    When Alice deletes the board "Personal Projects"
    Then Alice can no longer see "Personal Projects" in their list of boards
