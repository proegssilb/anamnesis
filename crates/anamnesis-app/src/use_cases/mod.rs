//! The use cases for the real domain model (`docs/DOMAIN.md`): orchestration
//! only. Each one resolves permission via `crate::policy`, loads through a
//! `crate::ports` trait, calls into `anamnesis_core`'s pure transitions, and
//! saves back through a port.

mod archive;
mod area;
mod attachment;
mod comment;
mod project;
mod relationship;
mod suggestion;
mod tangle;
mod task;

pub use archive::archive_done_tasks;
pub use area::{create_area, edit_area, list_areas, reposition_area, view_area};
pub use attachment::{
    add_file_attachment, add_link_attachment, delete_attachment, list_attachments,
};
pub use comment::{add_comment, delete_comment, edit_comment, list_comments};
pub use project::{
    add_field_definition, add_relationship_kind, archive_project, create_project, edit_project,
    edit_project_fields, list_projects_in_area, rename_field_definition,
    transition_project_status, unarchive_project, view_project,
};
pub use relationship::{create_relationship, delete_relationship, resolve_kind};
pub use suggestion::{derive_seed, request_suggestion};
pub use tangle::run_tangle_detection;
pub use task::{
    archive_task, create_task, drop_task, edit_task, raise_task, set_checklist_position,
    set_task_field_value, set_task_parent, unarchive_task, view_task,
};
