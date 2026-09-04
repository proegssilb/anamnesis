CREATE TABLE job_leases (
    job_name   TEXT    NOT NULL PRIMARY KEY,
    owner      TEXT    NOT NULL,
    expires_at INTEGER NOT NULL
);
