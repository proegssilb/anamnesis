#![forbid(unsafe_code)]
//! `anamnesis-core`: the pure domain model (`docs/DOMAIN.md`).
//!
//! No async, no I/O, no clock reads, no RNG: every function that needs "now"
//! or a freshly minted id takes it as a parameter. Every transition is
//! `fn(&Entity, ...) -> Result<Entity, DomainError>` — it reads the current
//! value and returns a brand new one, never mutating in place.
//!
//! `docs/DOMAIN.md` replaces the placeholder kanban model this crate used to
//! hold. That model — `Board`/`Column`/`Card` — lives on, unchanged, in
//! [`legacy`], purely so the other crates (rebuilt against this new model in
//! later phases) keep compiling in the meantime.

mod area;
mod column;
mod error;
mod field;
mod ids;
pub mod legacy;
mod placement;
pub mod policy;
mod project;
mod relationship;
mod task;
mod title;

pub use area::{Area, create_area, edit_area, reposition_area};
pub use column::{
    Column, create_column, rename_column, reposition_column, set_is_done, set_wip_limit,
};
pub use error::DomainError;
pub use field::{
    CurrencyAmount, CurrencyCode, FieldData, FieldDefinition, FieldKind, FieldValue, NumberValue,
    create_field_definition, rename_field_definition, set_field_position, set_field_value,
    set_show_on_card,
};
pub use ids::{
    AreaId, BoardId, CardId, ColumnId, FieldId, KindId, ProjectId, RelationshipId, TaskId,
    Timestamp, TimestampError, UserId,
};
pub use placement::Placement;
pub use project::{
    Project, ProjectStatus, archive_project, create_project, edit_project, transition_status,
    unarchive_project,
};
pub use relationship::{
    Relationship, RelationshipKind, builtin_blocks, builtin_duplicates, builtin_relates_to,
    create_relationship, create_relationship_kind, is_blocking,
};
pub use task::{
    Task, archive_task, create_task, edit_task, move_placement, set_checklist_position, set_parent,
    unarchive_task,
};
pub use title::{Title, TitleError};
