Feature: Tangles
  Tasks that mutually block each other form a knot. The system detects the
  whole knot as a single Tangle, however many blocking edges make it up, and
  clears it automatically the moment the block breaks -- without ever
  touching the tasks' own rows.

  Scenario: A knot of four mutually-blocking tasks becomes exactly one tangle
    Given the following tasks exist:
      | name |
      | A    |
      | B    |
      | C    |
      | D    |
    And "A" blocks "B"
    And "B" blocks "C"
    And "C" blocks "D"
    And "D" blocks "A"
    When tangles are detected
    Then exactly one tangle is detected
    And the tangle contains the following tasks:
      | name |
      | A    |
      | B    |
      | C    |
      | D    |

  Scenario: A tangle untangles automatically once the block breaks
    Given the following tasks exist:
      | name |
      | A    |
      | B    |
    And "A" blocks "B"
    And "B" blocks "A"
    And tangles have been detected and stored
    When "B" no longer blocks "A"
    And tangles are detected again
    Then no tangle is active any longer
    And the previously stored tangle is now marked resolved
