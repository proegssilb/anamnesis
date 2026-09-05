#![forbid(unsafe_code)]
//! `anamnesis-app`: the application layer for the real domain model
//! (`docs/DOMAIN.md`).
//!
//! [`ports`], [`use_cases`], [`policy`], and [`entities`] are the
//! application layer proper. [`ports::IdentityProvider`] (and its
//! `LoginRedirect`/`LoginCallback` companions) is the one piece of
//! infrastructure that predates this domain model and survives unchanged —
//! an OIDC login round trip is not part of `docs/DOMAIN.md` at all, it is
//! just genuinely shared infrastructure the web shell needs regardless of
//! which domain model sits behind it.
//!
//! The disposable kanban scaffold's own application layer (`Board`/`Column`/
//! `Card` ports and use cases) has been fully retired (Phase F1): every
//! crate now builds against the real domain model below.

pub mod access;
mod entities;
mod error;
pub mod policy;
mod ports;
mod settings;
mod use_cases;

pub use entities::{
    Attachment, AttachmentId, AttachmentKind, Comment, CommentId, attach_file, attach_link,
    create_comment, edit_comment as edit_comment_entity,
};
pub use error::{AppError, IdentityError, RepoError};
pub use ports::{
    AreaRepository, AttachmentRepository, AuthenticatedIdentity, BlobStore, BoardColumn, BoardItem,
    BoardQuery, Clock, CommentRepository, GroupMembershipQuery, GroupMembershipRepository, IdGen,
    IdentityProvider, JobLease, LoginCallback, LoginRedirect, MembershipQuery,
    MembershipRepository, ProjectAggregate, ProjectRepository, RelationshipRepository, SearchHit,
    SearchIndex, SearchQuery, SettingsRepository, TangleRepository, TaskAggregate, TaskRepository,
    TaskUpdateError, TimezoneResolver,
};
pub use settings::{
    DEFAULT_ACTIVE_PROJECT_LIMIT, DEFAULT_HIGH_BOUNCE_THRESHOLD,
    DEFAULT_SUGGESTION_COOLDOWN_SECONDS, Settings,
};
pub use use_cases::{
    ArchiveOutcome, BoardItemKind, add_comment, add_field_definition, add_file_attachment,
    add_link_attachment, add_relationship_kind, archive_done_tasks, archive_project, archive_task,
    create_area, create_project, create_relationship, create_task, delete_attachment,
    delete_comment, delete_relationship, derive_seed, drop_tangle, drop_task, edit_area,
    edit_comment, edit_project, edit_project_fields, edit_task, grant_admin_group,
    grant_area_group_role, grant_area_role, grant_project_group_role, grant_project_role,
    grant_system_admin, list_admin_groups, list_all_projects, list_area_groups, list_area_members,
    list_areas, list_attachments, list_comments, list_known_groups, list_project_groups,
    list_project_members, list_projects_in_area, list_system_admins, place_tangle, raise_task,
    rename_field_definition, reposition_area, reposition_board_item, request_suggestion,
    resolve_frozen_tangles, resolve_kind, revoke_admin_group, revoke_area_group_role,
    revoke_area_role, revoke_project_group_role, revoke_project_role, revoke_system_admin,
    run_tangle_detection, set_checklist_position, set_task_field_value, set_task_parent,
    transition_project_status, unarchive_project, unarchive_task, update_settings, view_area,
    view_project, view_settings, view_task,
};
