//! The use cases for the real domain model (`docs/DOMAIN.md`): orchestration
//! only. Each one resolves permission via `crate::policy`, loads through a
//! `crate::ports` trait, calls into `anamnesis_core`'s pure transitions, and
//! saves back through a port.

mod archive;
mod area;
mod attachment;
mod board;
mod comment;
mod indexing;
mod membership;
mod project;
mod relationship;
mod settings;
mod suggestion;
mod tangle;
mod task;

pub use archive::{ArchiveOutcome, archive_done_tasks};
pub use area::{create_area, edit_area, list_areas, reposition_area, view_area};
pub use attachment::{
    add_file_attachment, add_link_attachment, delete_attachment, list_attachments,
};
pub use board::{BoardItemKind, reposition_board_item};
pub use comment::{add_comment, delete_comment, edit_comment, list_comments};
pub use membership::{
    grant_area_role, grant_project_role, grant_system_admin, list_area_members,
    list_project_members, list_system_admins, revoke_area_role, revoke_project_role,
    revoke_system_admin,
};
pub use project::{
    add_field_definition, add_relationship_kind, archive_project, create_project, edit_project,
    edit_project_fields, list_all_projects, list_projects_in_area, rename_field_definition,
    transition_project_status, unarchive_project, view_project,
};
pub use relationship::{create_relationship, delete_relationship, resolve_kind};
pub use settings::{update_settings, view_settings};
pub use suggestion::{derive_seed, request_suggestion};
pub use tangle::{drop_tangle, place_tangle, resolve_frozen_tangles, run_tangle_detection};
pub use task::{
    archive_task, create_task, drop_task, edit_task, raise_task, set_checklist_position,
    set_task_field_value, set_task_parent, unarchive_task, view_task,
};
