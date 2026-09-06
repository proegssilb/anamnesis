//! Resolving a known user id to the display name it last presented — closes
//! two real usability gaps left by having no users table
//! (`docs/CONTEXT.md`): granting a role required an admin to already know
//! the target's raw OIDC `sub` by heart, since there was nothing to look it
//! up against, and a comment's author could only ever be rendered as that
//! same raw id.
//!
//! Neither gap was a security problem — `crate::ports::UserDirectoryQuery`'s
//! doc comment is explicit that a recorded name confers nothing — so unlike
//! `crate::use_cases::membership`, nothing here is about authorization.
//! Resolving names for ids a caller can already see (a comment it can
//! already view, a members list it can already list) needs no use case at
//! all — `crate::handlers::tasks::page` and `crate::handlers::
//! group_membership` call `UserDirectoryQuery::display_names` directly, the
//! same way they already call other ports purely to assemble a page.
//! [`list_known_users`] is the one exception: it is gated, because it is a
//! list of *everyone who has ever accessed this deployment*, which is
//! information a plain Member has no business seeing — the same reasoning
//! `crate::use_cases::group_membership::list_known_groups` states for its
//! own picker.

use anamnesis_core::UserId;
use anamnesis_core::policy::Role;

use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::UserDirectoryQuery;

/// Every user id this deployment has ever recorded a login for, with its
/// last-seen display name — the picker behind a "grant a role" user-id
/// input. Gated at the weakest tier that can create any grant
/// (`Action::ManageArea`), matching `list_known_groups`, since a Project
/// Admin needs it to fill in an area- or project-scoped grant form.
pub async fn list_known_users(
    query: &dyn UserDirectoryQuery,
    actor_role: Option<Role>,
) -> Result<Vec<(UserId, String)>, AppError> {
    if !is_allowed(actor_role, Action::ManageArea) {
        return Err(AppError::Forbidden);
    }
    Ok(query.list_known_users().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        known: Vec<(UserId, String)>,
    }

    #[async_trait::async_trait]
    impl UserDirectoryQuery for Recorder {
        async fn display_names(
            &self,
            _users: &[UserId],
        ) -> Result<std::collections::HashMap<UserId, String>, crate::error::RepoError> {
            Ok(std::collections::HashMap::new())
        }

        async fn list_known_users(&self) -> Result<Vec<(UserId, String)>, crate::error::RepoError> {
            Ok(self.known.clone())
        }
    }

    #[tokio::test]
    async fn a_member_cannot_list_known_users() {
        let query = Recorder::default();
        let result = list_known_users(&query, Some(Role::Member)).await;
        assert_eq!(result, Err(AppError::Forbidden));
    }

    #[tokio::test]
    async fn no_role_at_all_cannot_list_known_users() {
        let query = Recorder::default();
        let result = list_known_users(&query, None).await;
        assert_eq!(result, Err(AppError::Forbidden));
    }

    #[tokio::test]
    async fn a_project_admin_can_list_known_users() {
        let query = Recorder {
            known: vec![(UserId::new("alice"), "Alice".to_string())],
        };
        let result = list_known_users(&query, Some(Role::ProjectAdmin))
            .await
            .unwrap();
        assert_eq!(result, vec![(UserId::new("alice"), "Alice".to_string())]);
    }
}
