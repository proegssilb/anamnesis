-- Project-level sync with an external issue tracker (issues #40/#41): at
-- most one external repo per project, plus one Task <-> external Issue link
-- per synced task. See `anamnesis_app::sync`'s module doc comment for the
-- domain shape and `anamnesis_app::use_cases::sync`'s for the reconciliation
-- algorithm these tables back.
--
-- `base_url` is NULL only for a project synced against github.com --
-- required for GitHub Enterprise and every Forgejo instance (self-hosted,
-- no public host to default to). Validated in
-- `anamnesis_app::sync::configure_project_sync`/`edit_project_sync_config`,
-- not a CHECK constraint, so it stays testable without a database.
--
-- `encrypted_token` is `anamnesis_app::ports::TokenCipher::encrypt`'s output
-- (nonce || ciphertext, produced by `anamnesis_adapters::AesGcmTokenCipher`)
-- -- the plaintext personal access token is never stored.
CREATE TABLE project_sync_configs (
    project_id TEXT PRIMARY KEY REFERENCES projects (id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    base_url TEXT,
    owner TEXT NOT NULL,
    repo TEXT NOT NULL,
    encrypted_token BLOB NOT NULL,
    enabled INTEGER NOT NULL,
    auto_import_new_issues INTEGER NOT NULL,
    auto_push_new_tasks INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_synced_at INTEGER,
    last_sync_error TEXT
);

-- `list_enabled` (the reconciliation ticker's worklist) filters on this.
CREATE INDEX project_sync_configs_enabled_idx ON project_sync_configs (enabled);

-- One row per Task <-> external Issue link. `task_id` is the primary key: a
-- task syncs to at most one issue. `(project_id, external_issue_number)` is
-- unique so a pulled remote issue's "already imported?" check is one query
-- -- issue numbers are per-repo, not globally unique, hence scoping by
-- `project_id` rather than trusting the number alone.
CREATE TABLE task_sync_links (
    task_id TEXT PRIMARY KEY REFERENCES tasks (id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    external_issue_number INTEGER NOT NULL,
    external_url TEXT NOT NULL,
    last_remote_updated_at INTEGER NOT NULL,
    last_local_synced_at INTEGER NOT NULL,
    last_comment_synced_at INTEGER,
    created_at INTEGER NOT NULL,
    UNIQUE (project_id, external_issue_number)
);

CREATE INDEX task_sync_links_project_id_idx ON task_sync_links (project_id);

-- Comment origin annotation (issues #40/#41): a comment imported from an
-- external tracker carries all four of these columns; a locally authored
-- one carries none -- there is no partial state, so four nullable columns
-- rather than a separate table, and every existing read of `comments`
-- already loads one row per comment, so a join would add cost for nothing.
-- Dedup is scoped to `task_id`, not global, so two independent self-hosted
-- Forgejo instances can never collide on the same numeric comment id.
ALTER TABLE comments ADD COLUMN external_provider TEXT;
ALTER TABLE comments ADD COLUMN external_comment_id INTEGER;
ALTER TABLE comments ADD COLUMN external_url TEXT;
ALTER TABLE comments ADD COLUMN external_author_display TEXT;

CREATE UNIQUE INDEX comments_external_comment_dedup_idx
    ON comments (task_id, external_comment_id)
    WHERE external_comment_id IS NOT NULL;
