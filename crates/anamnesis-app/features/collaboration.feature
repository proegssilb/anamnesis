Feature: Collaborating on a task
  Comments and attachments append to a task (docs/DOMAIN.md SS3, SS7:
  "append-heavy, rarely all needed at once"). Editing or deleting someone
  else's comment needs Project Admin authority; editing your own never
  does, whatever role you hold (`crate::policy::can_edit_comment`).
  Deleting a file attachment also frees its blob; a link attachment
  carries nothing else to clean up.

  Scenario: A member can comment on a task and edit their own comment
    Given a task "Pick tile" below the horizon in project "Kitchen Remodel"
    And "Alice" is a Member of "Kitchen Remodel"
    When "Alice" comments "Let's go with the blue tile" on task "Pick tile"
    Then task "Pick tile" has 1 comment
    When "Alice" edits her comment on task "Pick tile" to "Let's go with the grey tile"
    Then access is granted
    And the comment on task "Pick tile" reads "Let's go with the grey tile"

  Scenario: A member cannot edit someone else's comment, but a project admin can
    Given a task "Pick tile" below the horizon in project "Kitchen Remodel"
    And "Alice" is a Member of "Kitchen Remodel"
    And "Bob" is a Member of "Kitchen Remodel"
    And "Priya" is a Project Admin of "Kitchen Remodel"
    When "Alice" comments "Let's go with the blue tile" on task "Pick tile"
    And "Bob" tries to edit Alice's comment on task "Pick tile" to "Let's go with the pink tile"
    Then access is refused
    When "Priya" tries to edit Alice's comment on task "Pick tile" to "Let's go with the pink tile"
    Then access is granted
    And the comment on task "Pick tile" reads "Let's go with the pink tile"

  Scenario: Attaching a link needs no blob; deleting a file attachment frees its blob
    Given a task "Pick tile" below the horizon in project "Kitchen Remodel"
    And "Alice" is a Member of "Kitchen Remodel"
    When "Alice" attaches the link "https://example.com/tile-samples" to task "Pick tile"
    And "Alice" attaches a file "grout-quote.pdf" to task "Pick tile"
    Then task "Pick tile" has 2 attachments
    And the file "grout-quote.pdf" is stored in the blob store
    When "Alice" deletes the file attachment "grout-quote.pdf" from task "Pick tile"
    Then task "Pick tile" has 1 attachment
    And the file "grout-quote.pdf" is no longer stored in the blob store
