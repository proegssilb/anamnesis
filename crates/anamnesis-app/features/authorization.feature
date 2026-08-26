Feature: Authorization
  A board belongs to the person who created it. Nobody else can look at it
  or change it — not even to add or move a single card.

  Background:
    Given Alice is signed in
    And Bob is signed in
    And Alice has a board named "Alice's Board" with a column named "To Do"
    And "To Do" on "Alice's Board" has the following cards in order:
      | title        |
      | Alice's card |

  Scenario Outline: A second user can neither read nor mutate someone else's board
    When Bob tries to <action> on "Alice's Board"
    Then Bob is forbidden

    Examples:
      | action           |
      | view the board   |
      | add a column     |
      | add a card       |
      | move a card      |
      | edit a card      |
      | delete a card    |
      | delete the board |

  Scenario: Board listings never mix users
    Given Bob has a board named "Bob's Board"
    Then Bob can see "Bob's Board" in their list of boards
    And Bob cannot see "Alice's Board" in their list of boards
    And Alice can see "Alice's Board" in their list of boards
    And Alice cannot see "Bob's Board" in their list of boards
