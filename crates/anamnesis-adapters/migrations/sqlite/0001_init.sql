-- The real domain model (docs/DOMAIN.md §3, §7). Per-entity tables, targeted
-- updates only -- no whole-aggregate delete-and-reinsert anywhere in this
-- file's consumers. `position`/`checklist_position`/board position columns
-- are written from the in-memory `Vec`/list index and read back with
-- `ORDER BY position` (or the task's own placement position).
--
-- UUIDs are stored as TEXT here (SQLite has no native UUID type); the
-- Postgres sibling of this migration uses native `uuid` instead.
--
-- The global task-board column entity is named `board_columns` here (not
-- `columns`) to leave room for a possible future per-project `columns`
-- concept without a name collision -- `docs/DOMAIN.md` §3 is explicit that
-- board columns are global, distinct from a project's own status lanes.

CREATE TABLE areas (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    position INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    area_id TEXT NOT NULL REFERENCES areas (id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    archived_at INTEGER
);

CREATE INDEX projects_area_id_idx ON projects (area_id);

CREATE TABLE field_definitions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    position INTEGER NOT NULL,
    show_on_card INTEGER NOT NULL
);

CREATE INDEX field_definitions_project_id_idx ON field_definitions (project_id);

-- Only project-local custom kinds are ever stored: the three built-ins
-- (docs/DOMAIN.md §3) are fixed constants in `anamnesis_core`, never rows
-- here (see `ProjectRepository::insert_relationship_kind`'s doc comment).
CREATE TABLE relationship_kinds (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    forward_label TEXT NOT NULL,
    reverse_label TEXT NOT NULL
);

CREATE INDEX relationship_kinds_project_id_idx ON relationship_kinds (project_id);

CREATE TABLE board_columns (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    position INTEGER NOT NULL,
    wip_limit INTEGER,
    is_done INTEGER NOT NULL
);

CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    -- Placement (anamnesis_core::Placement): 'below' leaves column_id and
    -- board_position NULL; 'on_board' populates both.
    placement_kind TEXT NOT NULL,
    column_id TEXT REFERENCES board_columns (id) ON DELETE SET NULL,
    board_position INTEGER,
    parent_task_id TEXT REFERENCES tasks (id) ON DELETE SET NULL,
    checklist_position INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    last_touched_at INTEGER NOT NULL,
    archived_at INTEGER,
    bounce_count INTEGER NOT NULL,
    last_bounced_at INTEGER,
    last_offered_at INTEGER
);

CREATE INDEX tasks_project_id_idx ON tasks (project_id);
CREATE INDEX tasks_parent_task_id_idx ON tasks (parent_task_id);
CREATE INDEX tasks_column_id_idx ON tasks (column_id);

-- Typed EAV (docs/DOMAIN.md §3): separate value_int / value_text / value_ts
-- columns, never JSON, so both backends sort and filter natively. Which
-- column(s) are populated for a given row is determined by the owning
-- field_definitions.kind:
--   Number    -> value_int (units), value_num_scale
--   Currency  -> value_int (minor units), value_currency_code
--   Date      -> value_ts (days since the Unix epoch, via time::Date::to_julian_day
--                minus the Julian day of 1970-01-01)
--   Time      -> value_ts (seconds since local midnight)
--   DateTime  -> value_ts (Unix seconds)
--   Line/Block -> value_text
CREATE TABLE field_values (
    field_id TEXT NOT NULL REFERENCES field_definitions (id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    value_int INTEGER,
    value_num_scale INTEGER,
    value_currency_code TEXT,
    value_text TEXT,
    value_ts INTEGER,
    PRIMARY KEY (field_id, task_id)
);

CREATE INDEX field_values_task_id_idx ON field_values (task_id);

-- Edges live outside any project (docs/DOMAIN.md §3): kind_id is not a
-- foreign key into relationship_kinds because a built-in kind's id never
-- appears as a row there.
CREATE TABLE relationships (
    id TEXT PRIMARY KEY,
    from_task_id TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    to_task_id TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    kind_id TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX relationships_from_task_id_idx ON relationships (from_task_id);
CREATE INDEX relationships_to_task_id_idx ON relationships (to_task_id);
CREATE INDEX relationships_kind_id_idx ON relationships (kind_id);

-- `fingerprint` is not stored: `anamnesis_core::Fingerprint` exposes no
-- accessor to its inner u64 (only `Fingerprint::of`, a pure function over a
-- task-id set), so it is recomputed from `tangle_tasks` on every load
-- instead of persisted redundantly.
--
-- `placement_kind`/`column_id`/`board_position` mirror `tasks`' own
-- placement encoding exactly (a tangle occupies a column slot just like a
-- task, docs/DOMAIN.md's Tangle section); `frozen` is set the moment a
-- tangle is placed and cleared when it drops back below the horizon.
CREATE TABLE tangles (
    id TEXT PRIMARY KEY,
    detected_at INTEGER NOT NULL,
    resolved_at INTEGER,
    placement_kind TEXT NOT NULL DEFAULT 'below',
    column_id TEXT REFERENCES board_columns (id) ON DELETE SET NULL,
    board_position INTEGER,
    frozen INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX tangles_column_id_idx ON tangles (column_id);

CREATE TABLE tangle_tasks (
    tangle_id TEXT NOT NULL REFERENCES tangles (id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    PRIMARY KEY (tangle_id, task_id)
);

CREATE TABLE comments (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    author TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    edited_at INTEGER
);

CREATE INDEX comments_task_id_idx ON comments (task_id);

CREATE TABLE attachments (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    url TEXT,
    blob_key TEXT,
    filename TEXT,
    mime TEXT,
    size INTEGER,
    created_at INTEGER NOT NULL
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
    area_id TEXT NOT NULL REFERENCES areas (id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    PRIMARY KEY (user_id, area_id)
);

CREATE TABLE project_members (
    user_id TEXT NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
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

-- Global search (docs/DOMAIN.md §7): FTS5 virtual table, kept current by
-- `SearchIndex` and read by `SearchQuery`. `entity_kind`/`entity_id` are
-- stored as plain columns (not `UNINDEXED` is fine here too, but they are
-- never the match target) alongside the indexed `title`.
CREATE VIRTUAL TABLE search_documents USING fts5(
    entity_kind UNINDEXED,
    entity_id UNINDEXED,
    title
);
