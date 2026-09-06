-- See sqlite/0005_known_users.sql.
CREATE TABLE known_users (
    user_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL
);
