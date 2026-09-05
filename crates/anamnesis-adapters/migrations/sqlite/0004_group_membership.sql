-- Optional OIDC group support: a second, independent source of role grants.
--
-- `user_groups` caches the groups the identity provider asserted for a user
-- at their last login, replaced wholesale each time they log in. It carries
-- no foreign key on `user_id` for the same reason `system_admins` does not:
-- there is no users table, and a user id is whatever the configured OIDC
-- claim resolved to.
--
-- The other three tables mirror `system_admins`, `area_members` and
-- `project_members` exactly, keyed on a group name instead of a user id. A
-- `user_groups` row grants nothing on its own -- it only matters where a
-- System Admin has separately mapped that group to a role here.

CREATE TABLE user_groups (
    user_id TEXT NOT NULL,
    group_name TEXT NOT NULL,
    PRIMARY KEY (user_id, group_name)
);

-- Every role lookup joins from a group name back to the users holding it.
CREATE INDEX idx_user_groups_group_name ON user_groups (group_name);

CREATE TABLE system_admin_groups (group_name TEXT PRIMARY KEY);

CREATE TABLE area_group_members (
    group_name TEXT NOT NULL,
    area_id TEXT NOT NULL REFERENCES areas (id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    PRIMARY KEY (group_name, area_id)
);

CREATE TABLE project_group_members (
    group_name TEXT NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    PRIMARY KEY (group_name, project_id)
);
