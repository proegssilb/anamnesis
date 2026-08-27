#![forbid(unsafe_code)]
//! `anamnesis-app`: the application layer.
//!
//! Two application layers currently live in this crate side by side
//! (`docs/DOMAIN.md` §7, §10):
//!
//! - [`legacy`] — the disposable kanban scaffold's ports and use cases
//!   (`Board`/`Column`/`Card`), re-exported at the crate root unchanged so
//!   `anamnesis-web` keeps compiling against it. Phase F removes it.
//! - Everything else at the crate root — [`ports`], [`use_cases`],
//!   [`policy`], [`entities`] — is the real domain model's application
//!   layer this crate is being rebuilt around (`docs/DOMAIN.md`).
//!
//! `Clock` and `IdGen` are the one piece of genuinely shared infrastructure
//! between the two: declared once in [`ports`], re-exported from
//! [`legacy`] too.

mod entities;
mod error;
mod legacy;
pub mod policy;
mod ports;
mod use_cases;

pub use entities::{
    Attachment, AttachmentId, AttachmentKind, Comment, CommentId, attach_file, attach_link,
    create_comment, edit_comment as edit_comment_entity,
};
pub use error::{AppError, IdentityError, RepoError};
pub use legacy::{
    Board, BoardRepository, BoardSummary, IdentityProvider, LoginCallback, LoginRedirect, add_card,
    add_column, create_board, delete_board, delete_card, edit_card, list_boards, move_card,
    view_board,
};
pub use ports::{
    AreaRepository, AttachmentRepository, BlobStore, BoardColumn, BoardQuery, Clock,
    CommentRepository, IdGen, MembershipQuery, ProjectAggregate, ProjectRepository,
    RelationshipRepository, SearchHit, SearchIndex, SearchQuery, TangleRepository, TaskAggregate,
    TaskRepository, TaskUpdateError, TimezoneResolver,
};
pub use use_cases::{
    add_comment, add_field_definition, add_file_attachment, add_link_attachment,
    add_relationship_kind, archive_done_tasks, archive_project, archive_task, create_area,
    create_project, create_relationship, create_task, delete_attachment, delete_comment,
    delete_relationship, derive_seed, drop_task, edit_area, edit_comment, edit_project,
    edit_project_fields, edit_task, list_areas, list_attachments, list_comments,
    list_projects_in_area, raise_task, rename_field_definition, reposition_area,
    request_suggestion, resolve_kind, run_tangle_detection, set_checklist_position,
    set_task_field_value, set_task_parent, transition_project_status, unarchive_project,
    unarchive_task, view_area, view_project, view_task,
};
