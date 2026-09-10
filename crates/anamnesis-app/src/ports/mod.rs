//! Port traits for the real domain model (`docs/DOMAIN.md` §7): what the
//! `crate::use_cases` need from the world. Nothing in this module (or its
//! submodules) names a concrete database, HTTP, or storage crate — see the
//! crate-level `cargo tree` check in the Phase D report.

mod common;
mod group_membership;
mod identity;
mod infra;
mod issue_tracker;
mod membership;
mod query;
mod repository;
mod sync;
mod user_directory;

pub use common::{Clock, IdGen, TokenCipher};
pub use group_membership::{GroupMembershipQuery, GroupMembershipRepository};
pub use identity::{AuthenticatedIdentity, IdentityProvider, LoginCallback, LoginRedirect};
pub use infra::{
    BlobInfo, BlobStore, ByteStream, ChunkedUpload, JobLease, PartInfo, SearchIndex,
    TimezoneResolver,
};
pub use issue_tracker::{IssueEdit, IssueState, IssueTrackerClient, RemoteComment, RemoteIssue};
pub use membership::{MembershipQuery, MembershipRepository};
pub use query::{BoardColumn, BoardItem, BoardQuery, SearchHit, SearchQuery};
pub use repository::{
    AreaRepository, AttachmentRepository, AttachmentUploadRepository, CommentRepository,
    ProjectAggregate, ProjectRepository, RelationshipRepository, SettingsRepository,
    TangleRepository, TaskAggregate, TaskRepository, TaskUpdateError,
};
pub use sync::{ProjectSyncConfigRepository, TaskSyncLinkRepository};
pub use user_directory::{UserDirectoryQuery, UserDirectoryRepository};
