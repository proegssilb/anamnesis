//! The real permission matrix (`docs/DOMAIN.md` §3, §10: "Ports + use cases;
//! policy/roles").
//!
//! `anamnesis_core::policy` states three deliberately conservative
//! predicates and says outright that they are "the shape the doc does state
//! explicitly" — a starting point, not the matrix, because `docs/DOMAIN.md`
//! names the three roles (System Admin / Project Admin / Member) and *some*
//! of their responsibilities without spelling out every capability. This
//! module builds the full matrix on top of those three predicates, one
//! [`Action`] per capability the use cases actually gate, and every use case
//! in `crate::use_cases` calls [`is_allowed`] rather than re-deriving role
//! logic inline (exactly what core's `policy` doc comment asks of "the
//! use-case layer (Phase D)").
//!
//! ## Resolved ambiguities (stated, not silently assumed)
//!
//! `docs/DOMAIN.md` §3 is explicit about System Admin (users, global
//! settings, columns, limits) and Project Admin (a project's fields, kinds,
//! membership), but does not say who may *create* an Area or a Project, or
//! whether viewing an Area requires a role at all. Resolved here as:
//!
//! - **Areas are now a real membership scope, and gated exactly like their
//!   Project-level counterparts.** Areas were originally System-Admin-only
//!   territory because roles were only ever project-scoped and an Area has
//!   no natural per-project owner to hang a role from. That was wrong for
//!   any non-admin user (`ViewArea`) and made `CreateProject` chicken-and-egg
//!   (a project that does not exist yet has no project role to authorize its
//!   own creation). The fix: an Area can carry its own membership rows (see
//!   [`crate::ports::MembershipQuery::area_role`]), and a Project inherits
//!   its Area's role when it has no explicit project role of its own (see
//!   [`crate::ports::MembershipQuery::effective_role`]). `ViewArea` is now
//!   gated identically to `ViewProject` ([`can_view_project`] — any assigned
//!   role), and `ManageArea` identically to the Project-Admin tier
//!   ([`can_manage_project`]).
//! - **Creating a Project requires Project Admin (or System Admin) in its
//!   Area.** A project is structural, not a capture action, so it sits in
//!   the same tier as `EditProject`/`ManageFieldDefinitions` rather than
//!   with ordinary task work — the caller resolves the *Area's* effective
//!   role (via [`crate::ports::MembershipQuery::effective_area_role`], since
//!   there is no project yet to resolve a role *in*) and passes that in.
//!   Ordinary task work (create, edit, archive, move, checklist reparenting,
//!   field values, relationships, comments, attachments, "archive all",
//!   requesting a suggestion) is gated identically: any role assigned to the
//!   project, because that is the entire point of "Member" — "ordinary
//!   access to a project: view and work its tasks."
//! - **Editing or deleting someone else's comment is a Project Admin (or
//!   System Admin) action; editing your own is not gated by role at all.**
//!   This composition (ownership OR admin) needs the comment's author,
//!   which `anamnesis_core::policy::Role` does not carry — see
//!   [`can_edit_comment`], which takes it as a parameter instead of folding
//!   it into [`Action`].

use anamnesis_core::UserId;
use anamnesis_core::policy::{Role, can_manage_project, can_manage_system, can_view_project};

/// Every capability a use case gates behind a role. One variant per
/// distinct rule in [`is_allowed`] — several actions share a rule (see the
/// match arms), but are kept as separate variants so a call site names
/// exactly what it is checking, which is what a reviewer (or a future
/// change to just one of them) needs to see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    // --- Area: a real membership scope, gated like its Project
    // counterparts (see module doc comment). ---
    ViewArea,
    ManageArea,

    // --- Project: structural actions are Project/System Admin only. ---
    ViewProject,
    CreateProject,
    EditProject,
    ArchiveProject,
    TransitionProjectStatus,
    ManageFieldDefinitions,
    ManageRelationshipKinds,
    ManageProjectMembership,

    // --- Task: ordinary work, any assigned role. ---
    ViewTask,
    CreateTask,
    EditTask,
    ArchiveTask,
    MoveTaskPlacement,
    SetTaskParent,
    SetTaskFieldValue,
    CreateRelationship,
    DeleteRelationship,
    CreateComment,
    CreateAttachment,
    DeleteAttachment,
    RunArchiveAll,
    RequestSuggestion,

    // --- System: System Admin only. ---
    ManageSystemSettings,
    ManageColumns,
    ManageActiveProjectLimit,
    ManageUsers,
}

/// Whether `role` (the caller's *effective* role — see
/// [`crate::ports::MembershipQuery::effective_role`]) may perform `action`.
///
/// `role: None` means "no role assigned at all" (not a member of the
/// project, and not a System Admin) and is refused for every action, exactly
/// as `anamnesis_core::policy::can_view_project` already establishes for
/// viewing.
pub fn is_allowed(role: Option<Role>, action: Action) -> bool {
    use Action::*;
    match action {
        ManageSystemSettings | ManageColumns | ManageActiveProjectLimit | ManageUsers => {
            can_manage_system(role)
        }

        // Area-scoped, exactly like their Project-level counterparts below
        // (see module doc comment): the caller resolves the *Area's*
        // effective role and passes it in.
        ViewArea => can_view_project(role),
        ManageArea => can_manage_project(role),

        // `CreateProject` is structural (not ordinary task work), so it
        // sits with `EditProject` et al. rather than with `ViewProject` —
        // gated on Project Admin (or System Admin) *in the Area*, since a
        // project that does not exist yet has no project role of its own.
        CreateProject
        | EditProject
        | ArchiveProject
        | TransitionProjectStatus
        | ManageFieldDefinitions
        | ManageRelationshipKinds
        | ManageProjectMembership => can_manage_project(role),

        ViewProject => can_view_project(role),

        ViewTask | CreateTask | EditTask | ArchiveTask | MoveTaskPlacement | SetTaskParent
        | SetTaskFieldValue | CreateRelationship | DeleteRelationship | CreateComment
        | CreateAttachment | DeleteAttachment | RunArchiveAll | RequestSuggestion => {
            can_view_project(role)
        }
    }
}

/// Whether `editor` may edit or delete a comment authored by `author`:
/// either they wrote it themselves, or they hold Project Admin (or System
/// Admin) authority over the project it lives in. Kept separate from
/// [`is_allowed`]/[`Action`] because it needs the comment's author, which no
/// [`Role`] carries.
pub fn can_edit_comment(role: Option<Role>, author: &UserId, editor: &UserId) -> bool {
    author == editor || can_manage_project(role)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn admin() -> Option<Role> {
        Some(Role::SystemAdmin)
    }

    fn project_admin() -> Option<Role> {
        Some(Role::ProjectAdmin)
    }

    fn member() -> Option<Role> {
        Some(Role::Member)
    }

    fn none() -> Option<Role> {
        None
    }

    // --- System-only actions: SystemAdmin yes, everyone else no. ---

    #[rstest]
    #[case(Action::ManageSystemSettings)]
    #[case(Action::ManageColumns)]
    #[case(Action::ManageActiveProjectLimit)]
    #[case(Action::ManageUsers)]
    fn system_actions_are_system_admin_only(#[case] action: Action) {
        assert!(is_allowed(admin(), action));
        assert!(!is_allowed(project_admin(), action));
        assert!(!is_allowed(member(), action));
        assert!(!is_allowed(none(), action));
    }

    // --- Area-scoped actions: gated exactly like their Project-level
    // counterparts, not System-Admin-only (see module doc comment). ---

    #[test]
    fn viewing_an_area_is_open_to_any_assigned_role() {
        assert!(is_allowed(admin(), Action::ViewArea));
        assert!(is_allowed(project_admin(), Action::ViewArea));
        assert!(is_allowed(member(), Action::ViewArea));
        assert!(!is_allowed(none(), Action::ViewArea));
    }

    #[test]
    fn managing_an_area_requires_admin() {
        assert!(is_allowed(admin(), Action::ManageArea));
        assert!(is_allowed(project_admin(), Action::ManageArea));
        assert!(!is_allowed(member(), Action::ManageArea));
        assert!(!is_allowed(none(), Action::ManageArea));
    }

    // --- Project-admin actions: SystemAdmin and ProjectAdmin, not Member.
    // `CreateProject` lives here too: it is structural (gated on Project
    // Admin in the project's Area), not ordinary task work. ---

    #[rstest]
    #[case(Action::CreateProject)]
    #[case(Action::EditProject)]
    #[case(Action::ArchiveProject)]
    #[case(Action::TransitionProjectStatus)]
    #[case(Action::ManageFieldDefinitions)]
    #[case(Action::ManageRelationshipKinds)]
    #[case(Action::ManageProjectMembership)]
    fn project_admin_actions_require_admin(#[case] action: Action) {
        assert!(is_allowed(admin(), action));
        assert!(is_allowed(project_admin(), action));
        assert!(!is_allowed(member(), action));
        assert!(!is_allowed(none(), action));
    }

    // --- Ordinary task work: every assigned role, never None. ---

    #[rstest]
    #[case(Action::ViewProject)]
    #[case(Action::ViewTask)]
    #[case(Action::CreateTask)]
    #[case(Action::EditTask)]
    #[case(Action::ArchiveTask)]
    #[case(Action::MoveTaskPlacement)]
    #[case(Action::SetTaskParent)]
    #[case(Action::SetTaskFieldValue)]
    #[case(Action::CreateRelationship)]
    #[case(Action::DeleteRelationship)]
    #[case(Action::CreateComment)]
    #[case(Action::CreateAttachment)]
    #[case(Action::DeleteAttachment)]
    #[case(Action::RunArchiveAll)]
    #[case(Action::RequestSuggestion)]
    fn ordinary_task_work_is_open_to_every_assigned_role(#[case] action: Action) {
        assert!(is_allowed(admin(), action));
        assert!(is_allowed(project_admin(), action));
        assert!(is_allowed(member(), action));
        assert!(!is_allowed(none(), action));
    }

    // --- Comment ownership composition. ---

    #[test]
    fn a_member_may_edit_their_own_comment() {
        let alice = UserId::new("alice");
        assert!(can_edit_comment(member(), &alice, &alice));
    }

    #[test]
    fn a_member_may_not_edit_someone_elses_comment() {
        let alice = UserId::new("alice");
        let bob = UserId::new("bob");
        assert!(!can_edit_comment(member(), &alice, &bob));
    }

    #[test]
    fn a_project_admin_may_edit_anyones_comment() {
        let alice = UserId::new("alice");
        let bob = UserId::new("bob");
        assert!(can_edit_comment(project_admin(), &alice, &bob));
    }

    #[test]
    fn a_system_admin_may_edit_anyones_comment() {
        let alice = UserId::new("alice");
        let bob = UserId::new("bob");
        assert!(can_edit_comment(admin(), &alice, &bob));
    }

    #[test]
    fn a_non_member_may_not_edit_someone_elses_comment() {
        let alice = UserId::new("alice");
        let bob = UserId::new("bob");
        assert!(!can_edit_comment(none(), &alice, &bob));
    }
}
