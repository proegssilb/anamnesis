-- Board aggregate: boards own columns, columns own cards. `position` is
-- written from the in-memory `Vec` index on every save and is the sole
-- source of ordering on read (`ORDER BY position`).
--
-- UUIDs are native `uuid` here (unlike the TEXT columns SQLite uses). `save`
-- writes the whole aggregate in one transaction, deleting and reinserting
-- columns and cards, so row identity (not row order) is what must survive a
-- save.

CREATE TABLE boards (
    id UUID PRIMARY KEY,
    owner TEXT NOT NULL,
    title TEXT NOT NULL
);

CREATE TABLE columns (
    id UUID PRIMARY KEY,
    board_id UUID NOT NULL REFERENCES boards (id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    wip_limit INTEGER,
    position INTEGER NOT NULL
);

CREATE INDEX columns_board_id_idx ON columns (board_id);

CREATE TABLE cards (
    id UUID PRIMARY KEY,
    column_id UUID NOT NULL REFERENCES columns (id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    position INTEGER NOT NULL
);

CREATE INDEX cards_column_id_idx ON cards (column_id);
