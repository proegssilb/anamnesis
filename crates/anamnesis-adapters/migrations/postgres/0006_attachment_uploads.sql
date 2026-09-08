-- See sqlite/0006_attachment_uploads.sql.
CREATE TABLE attachment_uploads (
    id UUID PRIMARY KEY,
    task_id UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    blob_key TEXT NOT NULL,
    storage_token TEXT NOT NULL,
    filename TEXT NOT NULL,
    mime TEXT NOT NULL,
    bytes_received BIGINT NOT NULL DEFAULT 0,
    created_by TEXT NOT NULL,
    created_at BIGINT NOT NULL
);

CREATE INDEX attachment_uploads_task_id_idx ON attachment_uploads (task_id);
CREATE INDEX attachment_uploads_created_at_idx ON attachment_uploads (created_at);

CREATE TABLE attachment_upload_parts (
    upload_id UUID NOT NULL REFERENCES attachment_uploads (id) ON DELETE CASCADE,
    part_number INTEGER NOT NULL,
    content_id TEXT NOT NULL,
    size BIGINT NOT NULL,
    PRIMARY KEY (upload_id, part_number)
);
