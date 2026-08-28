-- A resolved tangle sitting in a Done column had no way to ever be swept:
-- `Tangle` carried no `archived_at` of its own, so "Archive all" and the
-- scheduled sweep (both of which only ever touched `tasks`) left it on the
-- board forever. `anamnesis_core::archive_tangle` is the pure transition;
-- this is where it lands in storage.
ALTER TABLE tangles ADD COLUMN archived_at INTEGER;
