Feature: Moving cards
  Cards move within a column to reorder work, and across columns to track
  progress. A column with a work-in-progress limit will not accept a card
  once it is full — unless the card is already in that column and is only
  being reordered.

  Scenario: Reordering a card within its column
    Given Alice is signed in
    And Alice has a board named "Sprint" with a column named "To Do"
    And "To Do" on "Sprint" has the following cards in order:
      | title |
      | A     |
      | B     |
      | C     |
      | D     |
    When Alice moves the card "A" on "Sprint" to position 2 in "To Do"
    Then "To Do" on "Sprint" has the following cards in order:
      | title |
      | B     |
      | C     |
      | A     |
      | D     |

  Scenario: Moving a card past the end of a column lands it at the end
    Given Alice is signed in
    And Alice has a board named "Sprint" with a column named "To Do"
    And "To Do" on "Sprint" has the following cards in order:
      | title |
      | A     |
      | B     |
    When Alice moves the card "A" on "Sprint" to position 99 in "To Do"
    Then "To Do" on "Sprint" has the following cards in order:
      | title |
      | B     |
      | A     |

  Scenario: Moving a card into a different column
    Given Alice is signed in
    And Alice has a board named "Sprint" with a column named "To Do"
    And Alice has a board named "Sprint" with a column named "Doing"
    And "To Do" on "Sprint" has the following cards in order:
      | title |
      | A     |
    When Alice moves the card "A" on "Sprint" to position 0 in "Doing"
    Then "To Do" on "Sprint" has no cards
    And "Doing" on "Sprint" has the following cards in order:
      | title |
      | A     |

  Scenario: A full work-in-progress column rejects a card moving in from elsewhere
    Given Alice is signed in
    And Alice has a board named "Sprint" with a column named "To Do"
    And Alice has a board named "Sprint" with a column named "Doing" with a work-in-progress limit of 1
    And "Doing" on "Sprint" has the following cards in order:
      | title    |
      | Existing |
    And "To Do" on "Sprint" has the following cards in order:
      | title |
      | A     |
    When Alice tries to move the card "A" on "Sprint" to position 0 in "Doing"
    Then Alice is told the column is full
    And "Doing" on "Sprint" has the following cards in order:
      | title    |
      | Existing |

  Scenario: Reordering within a full work-in-progress column is still allowed
    Given Alice is signed in
    And Alice has a board named "Sprint" with a column named "Doing" with a work-in-progress limit of 2
    And "Doing" on "Sprint" has the following cards in order:
      | title |
      | X     |
      | Y     |
    When Alice moves the card "X" on "Sprint" to position 1 in "Doing"
    Then "Doing" on "Sprint" has the following cards in order:
      | title |
      | Y     |
      | X     |
