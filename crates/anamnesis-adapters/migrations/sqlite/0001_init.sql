-- Board aggregate: boards own columns, columns own cards. `position` is
-- written from the in-memory `Vec` index on every save and is the sole
-- source of ordering on read (`ORDER BY position`).
--
-- UUIDs are stored as TEXT in SQLite (no native UUID type); the adapter
-- parses/formats them at the boundary. `save` writes the whole aggregate in
-- one transaction, deleting and reinserting columns and cards, so row
-- identity (not row order) is what must survive a save.

CREATE TABLE boards (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    title TEXT NOT NULL
);

CREATE TABLE columns (
    id TEXT PRIMARY KEY,
    board_id TEXT NOT NULL REFERENCES boards (id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    wip_limit INTEGER,
    position INTEGER NOT NULL
);

CREATE INDEX columns_board_id_idx ON columns (board_id);

CREATE TABLE cards (
    id TEXT PRIMARY KEY,
    column_id TEXT NOT NULL REFERENCES columns (id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    position INTEGER NOT NULL
);

CREATE INDEX cards_column_id_idx ON cards (column_id);
