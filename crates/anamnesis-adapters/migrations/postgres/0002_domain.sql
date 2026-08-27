-- The real domain model (docs/DOMAIN.md §3, §7) -- Postgres sibling of
-- migrations/sqlite/0002_domain.sql. Same logical schema; native `uuid`
-- instead of TEXT ids, `BOOLEAN` instead of INTEGER flags, and
-- `tsvector`/GIN instead of FTS5 for search.

CREATE TABLE areas (
    id UUID PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    position INTEGER NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE projects (
    id UUID PRIMARY KEY,
    area_id UUID NOT NULL REFERENCES areas (id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    archived_at BIGINT
);

CREATE INDEX projects_area_id_idx ON projects (area_id);

CREATE TABLE field_definitions (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    position INTEGER NOT NULL,
    show_on_card BOOLEAN NOT NULL
);

CREATE INDEX field_definitions_project_id_idx ON field_definitions (project_id);

-- Only project-local custom kinds are ever stored: the three built-ins
-- (docs/DOMAIN.md §3) are fixed constants in `anamnesis_core`, never rows
-- here (see `ProjectRepository::insert_relationship_kind`'s doc comment).
CREATE TABLE relationship_kinds (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    forward_label TEXT NOT NULL,
    reverse_label TEXT NOT NULL
);

CREATE INDEX relationship_kinds_project_id_idx ON relationship_kinds (project_id);

CREATE TABLE board_columns (
    id UUID PRIMARY KEY,
    title TEXT NOT NULL,
    position INTEGER NOT NULL,
    wip_limit INTEGER,
    is_done BOOLEAN NOT NULL
);

CREATE TABLE tasks (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    placement_kind TEXT NOT NULL,
    column_id UUID REFERENCES board_columns (id) ON DELETE SET NULL,
    board_position INTEGER,
    parent_task_id UUID REFERENCES tasks (id) ON DELETE SET NULL,
    checklist_position INTEGER NOT NULL,
    created_at BIGINT NOT NULL,
    last_touched_at BIGINT NOT NULL,
    archived_at BIGINT,
    bounce_count INTEGER NOT NULL,
    last_bounced_at BIGINT,
    last_offered_at BIGINT
);

CREATE INDEX tasks_project_id_idx ON tasks (project_id);
CREATE INDEX tasks_parent_task_id_idx ON tasks (parent_task_id);
CREATE INDEX tasks_column_id_idx ON tasks (column_id);

-- Typed EAV (docs/DOMAIN.md §3): separate value_int / value_text / value_ts
-- columns, never JSON, so both backends sort and filter natively. See the
-- SQLite sibling migration for exactly what populates each column per
-- FieldKind.
CREATE TABLE field_values (
    field_id UUID NOT NULL REFERENCES field_definitions (id) ON DELETE CASCADE,
    task_id UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    value_int BIGINT,
    value_num_scale INTEGER,
    value_currency_code TEXT,
    value_text TEXT,
    value_ts BIGINT,
    PRIMARY KEY (field_id, task_id)
);

CREATE INDEX field_values_task_id_idx ON field_values (task_id);

-- Edges live outside any project (docs/DOMAIN.md §3): kind_id is not a
-- foreign key into relationship_kinds because a built-in kind's id never
-- appears as a row there.
CREATE TABLE relationships (
    id UUID PRIMARY KEY,
    from_task_id UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    to_task_id UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    kind_id UUID NOT NULL,
    created_at BIGINT NOT NULL
);

CREATE INDEX relationships_from_task_id_idx ON relationships (from_task_id);
CREATE INDEX relationships_to_task_id_idx ON relationships (to_task_id);
CREATE INDEX relationships_kind_id_idx ON relationships (kind_id);

-- `fingerprint` is not stored: `anamnesis_core::Fingerprint` exposes no
-- accessor to its inner u64 (only `Fingerprint::of`, a pure function over a
-- task-id set), so it is recomputed from `tangle_tasks` on every load
-- instead of persisted redundantly.
CREATE TABLE tangles (
    id UUID PRIMARY KEY,
    detected_at BIGINT NOT NULL,
    resolved_at BIGINT
);

CREATE TABLE tangle_tasks (
    tangle_id UUID NOT NULL REFERENCES tangles (id) ON DELETE CASCADE,
    task_id UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    PRIMARY KEY (tangle_id, task_id)
);

CREATE TABLE comments (
    id UUID PRIMARY KEY,
    task_id UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    author TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    edited_at BIGINT
);

CREATE INDEX comments_task_id_idx ON comments (task_id);

CREATE TABLE attachments (
    id UUID PRIMARY KEY,
    task_id UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    url TEXT,
    blob_key TEXT,
    filename TEXT,
    mime TEXT,
    size BIGINT,
    created_at BIGINT NOT NULL
);

CREATE INDEX attachments_task_id_idx ON attachments (task_id);

-- Membership: System Admin is global; area/project roles back
-- MembershipQuery, with an explicit project role always beating an
-- inherited area role (crate::ports::MembershipQuery::effective_role).
CREATE TABLE system_admins (
    user_id TEXT PRIMARY KEY
);

CREATE TABLE area_members (
    user_id TEXT NOT NULL,
    area_id UUID NOT NULL REFERENCES areas (id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    PRIMARY KEY (user_id, area_id)
);

CREATE TABLE project_members (
    user_id TEXT NOT NULL,
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    PRIMARY KEY (user_id, project_id)
);

-- Singleton settings row (docs/DOMAIN.md §3). No port in `anamnesis-app`
-- consumes this table yet (Phase D defined no `SettingsRepository`), so no
-- adapter code reads or writes it in this phase -- it exists so the schema
-- is complete for whichever later phase adds that port.
CREATE TABLE settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    active_project_limit INTEGER NOT NULL,
    timezone TEXT NOT NULL,
    sweep_recurrence_kind TEXT NOT NULL,
    sweep_recurrence_n INTEGER,
    sweep_recurrence_weekday TEXT,
    sweep_recurrence_day INTEGER,
    suggestion_cooldown_seconds INTEGER NOT NULL,
    suggestion_high_bounce_threshold INTEGER NOT NULL
);

-- Global search (docs/DOMAIN.md §7): a generated `tsvector` column plus a
-- GIN index, kept current by `SearchIndex` and read by `SearchQuery`.
CREATE TABLE search_documents (
    entity_kind TEXT NOT NULL,
    entity_id UUID NOT NULL,
    title TEXT NOT NULL,
    tsv TSVECTOR GENERATED ALWAYS AS (to_tsvector('english', title)) STORED,
    PRIMARY KEY (entity_kind, entity_id)
);

CREATE INDEX search_documents_tsv_idx ON search_documents USING GIN (tsv);
