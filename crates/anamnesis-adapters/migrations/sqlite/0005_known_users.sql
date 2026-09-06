-- A best-effort cache of the display name each user id last presented at
-- login. Not a users table (`docs/CONTEXT.md` is explicit there will never
-- be one): it carries no credential, no role, and no foreign key from
-- anywhere -- exactly like `user_groups` (`0004_group_membership.sql`).
--
-- It exists to close two real usability gaps that having no users table
-- left open: granting a role required an admin to already know the
-- target's raw OIDC `sub` by heart, because there was nothing to look it
-- up against, and a comment's author could only ever be rendered as that
-- same raw id. Written unconditionally on every login (see
-- `anamnesis-web::handlers::login`), and safe to lose or rebuild -- the
-- next login repopulates it.
CREATE TABLE known_users (
    user_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL
);
