-- `SettingsRepository::record_sweep` needs a place to stamp when the
-- scheduled sweep last actually ran, so the ticker's catch-up logic
-- (anamnesis-web::sweep) can compute due-ness from real history instead of
-- process uptime -- a sweep missed while the server was down still fires on
-- the next boot instead of being silently skipped.
ALTER TABLE settings ADD COLUMN last_swept_at INTEGER;
