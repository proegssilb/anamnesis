Feature: Chunked attachment upload
  A file too large or unwise to send as one request instead goes up as a
  begin, one or more parts, then a complete — separate calls against
  `crate::ports::ChunkedUpload` and `crate::ports::AttachmentUploadRepository`,
  tracked as a `PendingUpload` until it becomes a real attachment or is
  discarded (docs/DOMAIN.md SS3, issue #21). Kept as its own feature,
  deliberately separate from collaboration.feature's single-request
  attachment scenarios.

  Scenario: A file uploaded across several parts becomes one attachment
    Given a task "Pick tile" below the horizon in project "Kitchen Remodel"
    And "Alice" is a Member of "Kitchen Remodel"
    When "Alice" begins uploading a file "walkthrough.mp4" to task "Pick tile"
    And "Alice" uploads part 1 of "walkthrough.mp4" with 3 bytes
    And "Alice" uploads part 2 of "walkthrough.mp4" with 4 bytes
    And "Alice" completes uploading "walkthrough.mp4"
    Then task "Pick tile" has 1 attachment
    And the file "walkthrough.mp4" is stored in the blob store

  Scenario: Aborting a partial upload leaves no attachment and frees its storage
    Given a task "Pick tile" below the horizon in project "Kitchen Remodel"
    And "Alice" is a Member of "Kitchen Remodel"
    When "Alice" begins uploading a file "walkthrough.mp4" to task "Pick tile"
    And "Alice" uploads part 1 of "walkthrough.mp4" with 3 bytes
    And "Alice" aborts uploading "walkthrough.mp4"
    Then task "Pick tile" has 0 attachments

  Scenario: A part that pushes the upload over the attachment size cap is rejected
    Given a task "Pick tile" below the horizon in project "Kitchen Remodel"
    And "Alice" is a Member of "Kitchen Remodel"
    When "Alice" begins uploading a file "huge.bin" to task "Pick tile"
    And "Alice" uploads part 1 of "huge.bin" with 6 bytes
    And "Alice" tries to upload part 2 of "huge.bin" with 6 bytes against a 10 byte attachment cap
    Then the upload is rejected as too large
    And task "Pick tile" has 0 attachments

  Scenario: Only someone permitted to attach files can begin a chunked upload
    Given a task "Pick tile" below the horizon in project "Kitchen Remodel"
    When "Eve" (with no role) tries to begin uploading a file "walkthrough.mp4" to task "Pick tile"
    Then access is refused
