-- See sqlite/0007_project_sync.sql.
CREATE TABLE project_sync_configs (
    project_id UUID PRIMARY KEY REFERENCES projects (id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    base_url TEXT,
    owner TEXT NOT NULL,
    repo TEXT NOT NULL,
    encrypted_token BYTEA NOT NULL,
    enabled BOOLEAN NOT NULL,
    auto_import_new_issues BOOLEAN NOT NULL,
    auto_push_new_tasks BOOLEAN NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    last_synced_at BIGINT,
    last_sync_error TEXT
);

CREATE INDEX project_sync_configs_enabled_idx ON project_sync_configs (enabled);

CREATE TABLE task_sync_links (
    task_id UUID PRIMARY KEY REFERENCES tasks (id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    external_issue_number BIGINT NOT NULL,
    external_url TEXT NOT NULL,
    last_remote_updated_at BIGINT NOT NULL,
    last_local_synced_at BIGINT NOT NULL,
    last_comment_synced_at BIGINT,
    created_at BIGINT NOT NULL,
    UNIQUE (project_id, external_issue_number)
);

CREATE INDEX task_sync_links_project_id_idx ON task_sync_links (project_id);

ALTER TABLE comments ADD COLUMN external_provider TEXT;
ALTER TABLE comments ADD COLUMN external_comment_id BIGINT;
ALTER TABLE comments ADD COLUMN external_url TEXT;
ALTER TABLE comments ADD COLUMN external_author_display TEXT;

CREATE UNIQUE INDEX comments_external_comment_dedup_idx
    ON comments (task_id, external_comment_id)
    WHERE external_comment_id IS NOT NULL;
