//! Relationships between tasks (`docs/DOMAIN.md` §3). Edges live outside any
//! project, so `create_relationship` takes the two tasks' owning projects
//! explicitly (as `anamnesis_core::create_relationship` requires) rather
//! than resolving them from a single project-scoped role check.

use anamnesis_core::policy::Role;
use anamnesis_core::{
    self as core, KindId, Relationship, RelationshipId, RelationshipKind, TaskId,
};

use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{Clock, IdGen, ProjectRepository, RelationshipRepository};

/// Resolves a [`KindId`] to its [`RelationshipKind`]: the three built-in
/// kinds are fixed constants (`docs/DOMAIN.md` §3), checked first, so a
/// `ProjectRepository` implementation never needs to special-case them —
/// only project-local custom kinds are ever actually stored and looked up.
pub async fn resolve_kind(
    project_repo: &dyn ProjectRepository,
    id: KindId,
) -> Result<RelationshipKind, AppError> {
    if id == KindId::BUILTIN_BLOCKS {
        return Ok(core::builtin_blocks());
    }
    if id == KindId::BUILTIN_RELATES_TO {
        return Ok(core::builtin_relates_to());
    }
    if id == KindId::BUILTIN_DUPLICATES {
        return Ok(core::builtin_duplicates());
    }
    project_repo
        .load_relationship_kind(id)
        .await?
        .ok_or(AppError::NotFound)
}

/// Creates a relationship edge between two tasks.
#[allow(clippy::too_many_arguments)]
pub async fn create_relationship(
    relationship_repo: &dyn RelationshipRepository,
    project_repo: &dyn ProjectRepository,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    role: Option<Role>,
    from_task_id: TaskId,
    from_project_id: anamnesis_core::ProjectId,
    to_task_id: TaskId,
    to_project_id: anamnesis_core::ProjectId,
    kind_id: KindId,
) -> Result<Relationship, AppError> {
    if !is_allowed(role, Action::CreateRelationship) {
        return Err(AppError::Forbidden);
    }
    let kind = resolve_kind(project_repo, kind_id).await?;
    let relationship = core::create_relationship(
        RelationshipId::new(ids.next()),
        from_task_id,
        from_project_id,
        to_task_id,
        to_project_id,
        &kind,
        clock.now(),
    )?;
    relationship_repo.insert(&relationship).await?;
    Ok(relationship)
}

/// Deletes a relationship edge.
pub async fn delete_relationship(
    relationship_repo: &dyn RelationshipRepository,
    role: Option<Role>,
    id: RelationshipId,
) -> Result<(), AppError> {
    if !is_allowed(role, Action::DeleteRelationship) {
        return Err(AppError::Forbidden);
    }
    relationship_repo
        .load(id)
        .await?
        .ok_or(AppError::NotFound)?;
    relationship_repo.delete(id).await?;
    Ok(())
}
