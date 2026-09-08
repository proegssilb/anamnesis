-- Chunked file uploads in progress (issue #21's multi-request half): a
-- client begins an upload, PUTs one or more parts across separate HTTP
-- requests, then completes it. Tracked here -- not just in a `BlobStore`
-- adapter's own memory -- so a later part or the completion can be handled
-- by *any* instance sharing this database, not only the one that saw the
-- first request (see `anamnesis_app::ports::infra::ChunkedUpload`'s doc
-- comment for the stateless design this enables).
--
-- `blob_key` is pre-minted at `begin_file_upload` time, exactly like a
-- single-shot upload's blob key -- it becomes the finished `Attachment`'s
-- `blob_key` once `complete_file_upload` runs. `storage_token` is whatever
-- opaque token the configured `BlobStore` backend needs to resume the
-- upload (a real S3 multipart upload id, or an `FsBlobStore` staging
-- directory name).
CREATE TABLE attachment_uploads (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    blob_key TEXT NOT NULL,
    storage_token TEXT NOT NULL,
    filename TEXT NOT NULL,
    mime TEXT NOT NULL,
    bytes_received INTEGER NOT NULL DEFAULT 0,
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX attachment_uploads_task_id_idx ON attachment_uploads (task_id);

-- Only used to find abandoned uploads for the GC sweep
-- (`anamnesis_app::expire_stale_uploads`) -- `created_at` doubles as "last
-- activity" since nothing here is ever updated after a part lands (parts
-- land in `attachment_upload_parts`, not by touching this row).
CREATE INDEX attachment_uploads_created_at_idx ON attachment_uploads (created_at);

-- One row per successfully uploaded part. `etag` is a backend-specific
-- opaque manifest entry (an S3 part's real ETag; unused, always empty, for
-- `FsBlobStore`) that `complete_file_upload` hands back to the `BlobStore`
-- verbatim -- this table, not the adapter, is what makes completing an
-- upload possible from any instance regardless of which instance(s)
-- handled which parts.
CREATE TABLE attachment_upload_parts (
    upload_id TEXT NOT NULL REFERENCES attachment_uploads (id) ON DELETE CASCADE,
    part_number INTEGER NOT NULL,
    content_id TEXT NOT NULL,
    size INTEGER NOT NULL,
    PRIMARY KEY (upload_id, part_number)
);
