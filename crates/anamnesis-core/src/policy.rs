//! Role-aware authorization, generalising the placeholder model's single
//! `can_view` (owner-only) rule (`docs/DOMAIN.md` §3).
//!
//! `docs/DOMAIN.md` names three roles — System Admin (users, global
//! settings, columns, limits), Project Admin (a project's fields, kinds,
//! membership), Member — but does not fully spec every rule. This module
//! implements the shape the doc does state explicitly, as a small,
//! deliberately conservative set of checks; the use-case layer (Phase D) is
//! expected to call these rather than re-deriving role logic inline.
//!
//! A user's effective [`Role`] is *with respect to one project* (or, for
//! system-level actions, simply "are they a System Admin") — core has no
//! membership table to consult, so the caller (which does) resolves
//! "what role does this user hold here" before calling in.

/// A user's role, scoped to whatever resource is being checked.
///
/// Roles form a ladder — `Member < ProjectAdmin < SystemAdmin` — via
/// [`Ord`], derived from [`Role::rank`] rather than hand-written comparisons.
/// This is what lets independently-granted roles (e.g. an Area grant and a
/// Project grant on the same user) be composed by taking the strongest: see
/// `anamnesis_app::ports::MembershipQuery::effective_role`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Manages users, global settings, global columns, and the active
    /// project limit. Implicitly has every `ProjectAdmin` and `Member`
    /// permission on every project, since nothing in the system is meant to
    /// be hidden from the person operating it.
    SystemAdmin,
    /// Manages one project's field definitions, relationship kinds, and
    /// membership.
    ProjectAdmin,
    /// Ordinary access to a project: view and work its tasks.
    Member,
}

impl Role {
    /// This role's position on the ladder `Member < ProjectAdmin <
    /// SystemAdmin`, as a plain number so [`Ord`] can be derived from one
    /// place instead of drifting across hand-written if-chains.
    fn rank(self) -> u8 {
        match self {
            Role::Member => 0,
            Role::ProjectAdmin => 1,
            Role::SystemAdmin => 2,
        }
    }
}

impl PartialOrd for Role {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Role {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.rank().cmp(&other.rank())
    }
}

/// Whether a user with `role` in a project (`None` = not a member of it, and
/// not a System Admin) may view that project and its tasks.
///
/// Every role that exists at all can view — `None` is the only case that
/// cannot, mirroring the placeholder model's `can_view`, generalised from
/// "is the owner" to "holds any role here".
pub fn can_view_project(role: Option<Role>) -> bool {
    role.is_some()
}

/// Whether `role` may manage a project's field definitions, relationship
/// kinds, or membership.
pub fn can_manage_project(role: Option<Role>) -> bool {
    matches!(role, Some(Role::SystemAdmin) | Some(Role::ProjectAdmin))
}

/// Whether `role` may manage system-wide concerns: users, global settings,
/// global board columns, the active-project limit.
pub fn can_manage_system(role: Option<Role>) -> bool {
    matches!(role, Some(Role::SystemAdmin))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(Some(Role::SystemAdmin), true)]
    #[case(Some(Role::ProjectAdmin), true)]
    #[case(Some(Role::Member), true)]
    #[case(None, false)]
    fn can_view_project_holds_for_any_assigned_role(
        #[case] role: Option<Role>,
        #[case] expected: bool,
    ) {
        assert_eq!(can_view_project(role), expected);
    }

    #[rstest]
    #[case(Some(Role::SystemAdmin), true)]
    #[case(Some(Role::ProjectAdmin), true)]
    #[case(Some(Role::Member), false)]
    #[case(None, false)]
    fn can_manage_project_requires_admin(#[case] role: Option<Role>, #[case] expected: bool) {
        assert_eq!(can_manage_project(role), expected);
    }

    #[rstest]
    #[case(Some(Role::SystemAdmin), true)]
    #[case(Some(Role::ProjectAdmin), false)]
    #[case(Some(Role::Member), false)]
    #[case(None, false)]
    fn can_manage_system_requires_system_admin(#[case] role: Option<Role>, #[case] expected: bool) {
        assert_eq!(can_manage_system(role), expected);
    }

    #[test]
    fn role_ladder_orders_member_below_project_admin_below_system_admin() {
        assert!(Role::Member < Role::ProjectAdmin);
        assert!(Role::ProjectAdmin < Role::SystemAdmin);
        assert!(Role::Member < Role::SystemAdmin);
        assert_eq!(Role::Member.max(Role::ProjectAdmin), Role::ProjectAdmin);
        assert_eq!(Role::SystemAdmin.max(Role::ProjectAdmin), Role::SystemAdmin);
    }
}
