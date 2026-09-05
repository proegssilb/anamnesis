//! Port traits for the real domain model (`docs/DOMAIN.md` §7): what the
//! `crate::use_cases` need from the world. Nothing in this module (or its
//! submodules) names a concrete database, HTTP, or storage crate — see the
//! crate-level `cargo tree` check in the Phase D report.

mod common;
mod group_membership;
mod identity;
mod infra;
mod membership;
mod query;
mod repository;

pub use common::{Clock, IdGen};
pub use group_membership::{GroupMembershipQuery, GroupMembershipRepository};
pub use identity::{AuthenticatedIdentity, IdentityProvider, LoginCallback, LoginRedirect};
pub use infra::{BlobStore, JobLease, SearchIndex, TimezoneResolver};
pub use membership::{MembershipQuery, MembershipRepository};
pub use query::{BoardColumn, BoardItem, BoardQuery, SearchHit, SearchQuery};
pub use repository::{
    AreaRepository, AttachmentRepository, CommentRepository, ProjectAggregate, ProjectRepository,
    RelationshipRepository, SettingsRepository, TangleRepository, TaskAggregate, TaskRepository,
    TaskUpdateError,
};
