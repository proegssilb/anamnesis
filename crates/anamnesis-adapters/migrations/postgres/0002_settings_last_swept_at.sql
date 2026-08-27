-- See sqlite/0002_settings_last_swept_at.sql.
ALTER TABLE settings ADD COLUMN last_swept_at BIGINT;
