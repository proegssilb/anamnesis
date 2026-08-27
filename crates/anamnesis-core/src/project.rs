//! [`Project`]: a concrete saga or ongoing commitment (`docs/DOMAIN.md` §3).
//!
//! `FieldDefinition[]` and `RelationshipKind[]` are described in the design
//! doc as "loaded with" a project, but that is a repository/read-model
//! concern (§7: "Introduce read models (CQRS-lite)", "Repository ports
//! become per-entity") — Phase D territory. In this pure core, `Project` is a
//! flat entity; `FieldDefinition` and `RelationshipKind` are standalone
//! entities that merely reference `project_id`, composed by the query layer.

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::{AreaId, ProjectId, Timestamp};
use crate::title::Title;

/// A project's lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectStatus {
    Pending,
    Active,
    Complete,
}

/// A concrete saga or ongoing commitment, living within one [`Area`](crate::Area).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub area_id: AreaId,
    pub title: Title,
    pub description: String,
    pub status: ProjectStatus,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub archived_at: Option<Timestamp>,
}

/// Creates a new project in `Pending` status.
pub fn create_project(
    id: ProjectId,
    area_id: AreaId,
    title: impl AsRef<str>,
    description: impl Into<String>,
    now: Timestamp,
) -> Result<Project, DomainError> {
    let title = Title::new(title)?;
    Ok(Project {
        id,
        area_id,
        title,
        description: description.into(),
        status: ProjectStatus::Pending,
        created_at: now,
        updated_at: now,
        archived_at: None,
    })
}

/// Replaces a project's title and description, stamping `updated_at`.
pub fn edit_project(
    project: &Project,
    title: impl AsRef<str>,
    description: impl Into<String>,
    now: Timestamp,
) -> Result<Project, DomainError> {
    let title = Title::new(title)?;
    Ok(Project {
        title,
        description: description.into(),
        updated_at: now,
        ..project.clone()
    })
}

/// Transitions a project's status.
///
/// `active_count_excluding_self` is the number of *other* projects currently
/// `Active` — supplied by the caller, since core loads no collection to
/// count over. Enforces the global invariant from `docs/DOMAIN.md` §3:
/// `count(status == Active) <= settings.active_project_limit`. The check
/// only applies when the *new* status is `Active`; leaving `Active` never
/// needs it.
pub fn transition_status(
    project: &Project,
    new_status: ProjectStatus,
    active_count_excluding_self: u32,
    active_project_limit: u32,
    now: Timestamp,
) -> Result<Project, DomainError> {
    if matches!(new_status, ProjectStatus::Active)
        && active_count_excluding_self >= active_project_limit
    {
        return Err(DomainError::ActiveProjectLimitExceeded);
    }
    Ok(Project {
        status: new_status,
        updated_at: now,
        ..project.clone()
    })
}

/// Archives a project. Rejects an already-archived project.
pub fn archive_project(project: &Project, now: Timestamp) -> Result<Project, DomainError> {
    if project.archived_at.is_some() {
        return Err(DomainError::AlreadyArchived);
    }
    Ok(Project {
        archived_at: Some(now),
        updated_at: now,
        ..project.clone()
    })
}

/// Restores an archived project. Rejects a project that is not archived.
pub fn unarchive_project(project: &Project, now: Timestamp) -> Result<Project, DomainError> {
    if project.archived_at.is_none() {
        return Err(DomainError::NotArchived);
    }
    Ok(Project {
        archived_at: None,
        updated_at: now,
        ..project.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn pid(n: u128) -> ProjectId {
        ProjectId::new(Uuid::from_u128(n))
    }

    fn aid(n: u128) -> AreaId {
        AreaId::new(Uuid::from_u128(n))
    }

    fn ts(secs: i64) -> Timestamp {
        Timestamp::from_unix_seconds(secs).unwrap()
    }

    fn project() -> Project {
        create_project(pid(1), aid(1), "Kitchen remodel", "", ts(0)).unwrap()
    }

    #[test]
    fn create_project_starts_pending() {
        let p = project();
        assert_eq!(p.status, ProjectStatus::Pending);
        assert_eq!(p.area_id, aid(1));
        assert!(p.archived_at.is_none());
    }

    #[test]
    fn create_project_rejects_an_invalid_title() {
        let result = create_project(pid(1), aid(1), "", "", ts(0));
        assert!(matches!(result, Err(DomainError::InvalidTitle(_))));
    }

    #[test]
    fn edit_project_replaces_title_and_description() {
        let p = project();
        let edited = edit_project(&p, "New title", "new desc", ts(10)).unwrap();
        assert_eq!(edited.title.as_str(), "New title");
        assert_eq!(edited.description, "new desc");
        assert_eq!(edited.updated_at, ts(10));
    }

    #[test]
    fn transition_to_active_succeeds_under_the_limit() {
        let p = project();
        let result = transition_status(&p, ProjectStatus::Active, 2, 3, ts(10)).unwrap();
        assert_eq!(result.status, ProjectStatus::Active);
        assert_eq!(result.updated_at, ts(10));
    }

    #[test]
    fn transition_to_active_rejected_at_the_limit() {
        let p = project();
        let result = transition_status(&p, ProjectStatus::Active, 3, 3, ts(10));
        assert_eq!(result, Err(DomainError::ActiveProjectLimitExceeded));
    }

    #[test]
    fn transition_away_from_active_ignores_the_limit() {
        let mut p = project();
        p.status = ProjectStatus::Active;
        let result = transition_status(&p, ProjectStatus::Complete, 999, 1, ts(10)).unwrap();
        assert_eq!(result.status, ProjectStatus::Complete);
    }

    #[test]
    fn archive_project_stamps_archived_at() {
        let p = project();
        let archived = archive_project(&p, ts(20)).unwrap();
        assert_eq!(archived.archived_at, Some(ts(20)));
    }

    #[test]
    fn archive_project_rejects_an_already_archived_project() {
        let p = project();
        let archived = archive_project(&p, ts(20)).unwrap();
        let result = archive_project(&archived, ts(30));
        assert_eq!(result, Err(DomainError::AlreadyArchived));
    }

    #[test]
    fn unarchive_project_clears_archived_at() {
        let p = project();
        let archived = archive_project(&p, ts(20)).unwrap();
        let restored = unarchive_project(&archived, ts(30)).unwrap();
        assert!(restored.archived_at.is_none());
        assert_eq!(restored.updated_at, ts(30));
    }

    #[test]
    fn unarchive_project_rejects_a_project_that_is_not_archived() {
        let p = project();
        let result = unarchive_project(&p, ts(30));
        assert_eq!(result, Err(DomainError::NotArchived));
    }
}
