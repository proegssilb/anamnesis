-- See sqlite/0008_recurring_tasks.sql. Timestamps are BIGINT Unix seconds,
-- matching every other timestamp column in this schema.
ALTER TABLE tasks ADD COLUMN schedule_json TEXT;
ALTER TABLE tasks ADD COLUMN next_due_at BIGINT;
ALTER TABLE tasks ADD COLUMN expires_at BIGINT;
ALTER TABLE tasks ADD COLUMN last_completed_at BIGINT;
ALTER TABLE tasks ADD COLUMN due_since BIGINT;

CREATE INDEX tasks_next_due_at ON tasks (next_due_at) WHERE schedule_json IS NOT NULL;
CREATE INDEX tasks_expires_at ON tasks (expires_at) WHERE expires_at IS NOT NULL;

ALTER TABLE settings ADD COLUMN suggestion_due_slot_percent INTEGER NOT NULL DEFAULT 80;
