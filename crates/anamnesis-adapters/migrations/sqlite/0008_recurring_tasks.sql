-- Recurring tasks (household chores). `schedule_json` is the serde_json
-- encoding of `anamnesis_core::Schedule`; it is NULL exactly when the task
-- is not recurring. The four timestamps are Unix seconds like every other
-- timestamp column, all nullable (a paused chore has no `next_due_at`).
ALTER TABLE tasks ADD COLUMN schedule_json TEXT;
ALTER TABLE tasks ADD COLUMN next_due_at INTEGER;
ALTER TABLE tasks ADD COLUMN expires_at INTEGER;
ALTER TABLE tasks ADD COLUMN last_completed_at INTEGER;
ALTER TABLE tasks ADD COLUMN due_since INTEGER;

-- The recurrence ticker's worklist and sleep-until queries.
CREATE INDEX tasks_next_due_at ON tasks (next_due_at) WHERE schedule_json IS NOT NULL;
CREATE INDEX tasks_expires_at ON tasks (expires_at) WHERE expires_at IS NOT NULL;

ALTER TABLE settings ADD COLUMN suggestion_due_slot_percent INTEGER NOT NULL DEFAULT 80;
