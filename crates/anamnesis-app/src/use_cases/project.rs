//! Project CRUD, field definitions, and project-local relationship kinds
//! (`docs/DOMAIN.md` §3). Role is resolved by the caller (typically via
//! `crate::ports::MembershipQuery::effective_role`) and passed in — these
//! use cases only enforce, they never look membership up themselves, so a
//! `create_project` (which by definition has no project yet to resolve a
//! role *in*) can still be gated on the *area's* role.

use anamnesis_core::policy::Role;
use anamnesis_core::{
    self as core, AreaId, FieldDefinition, FieldKind, KindId, Project, ProjectId, ProjectStatus,
    RelationshipKind,
};

use crate::error::AppError;
use crate::policy::{Action, is_allowed};
use crate::ports::{Clock, IdGen, ProjectAggregate, ProjectRepository};

/// Creates a new project (in `Pending` status) within an area.
pub async fn create_project(
    repo: &dyn ProjectRepository,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    role: Option<Role>,
    area_id: AreaId,
    title: &str,
    description: &str,
) -> Result<Project, AppError> {
    if !is_allowed(role, Action::CreateProject) {
        return Err(AppError::Forbidden);
    }
    let project = core::create_project(
        ProjectId::new(ids.next()),
        area_id,
        title,
        description,
        clock.now(),
    )?;
    repo.insert(&project).await?;
    Ok(project)
}

/// Loads a project together with its field definitions and relationship
/// kinds (`docs/DOMAIN.md` §7).
pub async fn view_project(
    repo: &dyn ProjectRepository,
    role: Option<Role>,
    id: ProjectId,
) -> Result<ProjectAggregate, AppError> {
    if !is_allowed(role, Action::ViewProject) {
        return Err(AppError::Forbidden);
    }
    repo.load(id).await?.ok_or(AppError::NotFound)
}

/// Lists every (non-aggregate) project within an area.
pub async fn list_projects_in_area(
    repo: &dyn ProjectRepository,
    role: Option<Role>,
    area_id: AreaId,
) -> Result<Vec<Project>, AppError> {
    if !is_allowed(role, Action::ViewProject) {
        return Err(AppError::Forbidden);
    }
    Ok(repo.list_by_area(area_id).await?)
}

/// Replaces a project's title and description.
pub async fn edit_project(
    repo: &dyn ProjectRepository,
    clock: &dyn Clock,
    role: Option<Role>,
    id: ProjectId,
) -> Result<Project, AppError> {
    edit_project_fields(repo, clock, role, id, None, None).await
}

/// Replaces a project's title and/or description; `None` leaves a field
/// unchanged. Split from [`edit_project`] so callers that only want to
/// rename need not repeat the current description (and vice versa).
pub async fn edit_project_fields(
    repo: &dyn ProjectRepository,
    clock: &dyn Clock,
    role: Option<Role>,
    id: ProjectId,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<Project, AppError> {
    if !is_allowed(role, Action::EditProject) {
        return Err(AppError::Forbidden);
    }
    let aggregate = repo.load(id).await?.ok_or(AppError::NotFound)?;
    let project = aggregate.project;
    let new_title = title.unwrap_or(project.title.as_str());
    let new_description = description.unwrap_or(project.description.as_str());
    let edited = core::edit_project(&project, new_title, new_description, clock.now())?;
    repo.update(&edited).await?;
    Ok(edited)
}

/// Transitions a project's status, enforcing `docs/DOMAIN.md` §3's global
/// invariant (`count(status == Active) <= settings.active_project_limit`) by
/// asking the repository for the current count — the use-case-layer
/// enforcement `docs/DOMAIN.md` §7 requires, since `anamnesis_core` itself
/// can only check a count it is handed.
pub async fn transition_project_status(
    repo: &dyn ProjectRepository,
    clock: &dyn Clock,
    role: Option<Role>,
    id: ProjectId,
    new_status: ProjectStatus,
    active_project_limit: u32,
) -> Result<Project, AppError> {
    if !is_allowed(role, Action::TransitionProjectStatus) {
        return Err(AppError::Forbidden);
    }
    let aggregate = repo.load(id).await?.ok_or(AppError::NotFound)?;
    let active_count_excluding_self = repo.count_active(Some(id)).await?;
    let transitioned = core::transition_status(
        &aggregate.project,
        new_status,
        active_count_excluding_self,
        active_project_limit,
        clock.now(),
    )?;
    repo.update(&transitioned).await?;
    Ok(transitioned)
}

/// Archives a project.
pub async fn archive_project(
    repo: &dyn ProjectRepository,
    clock: &dyn Clock,
    role: Option<Role>,
    id: ProjectId,
) -> Result<Project, AppError> {
    if !is_allowed(role, Action::ArchiveProject) {
        return Err(AppError::Forbidden);
    }
    let aggregate = repo.load(id).await?.ok_or(AppError::NotFound)?;
    let archived = core::archive_project(&aggregate.project, clock.now())?;
    repo.update(&archived).await?;
    Ok(archived)
}

/// Restores an archived project.
pub async fn unarchive_project(
    repo: &dyn ProjectRepository,
    clock: &dyn Clock,
    role: Option<Role>,
    id: ProjectId,
) -> Result<Project, AppError> {
    if !is_allowed(role, Action::ArchiveProject) {
        return Err(AppError::Forbidden);
    }
    let aggregate = repo.load(id).await?.ok_or(AppError::NotFound)?;
    let restored = core::unarchive_project(&aggregate.project, clock.now())?;
    repo.update(&restored).await?;
    Ok(restored)
}

/// Adds a new field definition to a project.
#[allow(clippy::too_many_arguments)]
pub async fn add_field_definition(
    repo: &dyn ProjectRepository,
    ids: &dyn IdGen,
    role: Option<Role>,
    project_id: ProjectId,
    name: &str,
    kind: FieldKind,
    position: u32,
    show_on_card: bool,
) -> Result<FieldDefinition, AppError> {
    if !is_allowed(role, Action::ManageFieldDefinitions) {
        return Err(AppError::Forbidden);
    }
    // Ensures the project exists before adding to its vocabulary.
    repo.load(project_id).await?.ok_or(AppError::NotFound)?;
    let definition = core::create_field_definition(
        ids.next().into(),
        project_id,
        name,
        kind,
        position,
        show_on_card,
    )?;
    repo.insert_field_definition(&definition).await?;
    Ok(definition)
}

/// Renames a field definition.
pub async fn rename_field_definition(
    repo: &dyn ProjectRepository,
    role: Option<Role>,
    project_id: ProjectId,
    field_id: anamnesis_core::FieldId,
    name: &str,
) -> Result<FieldDefinition, AppError> {
    if !is_allowed(role, Action::ManageFieldDefinitions) {
        return Err(AppError::Forbidden);
    }
    let aggregate = repo.load(project_id).await?.ok_or(AppError::NotFound)?;
    let definition = aggregate
        .field_definitions
        .into_iter()
        .find(|d| d.id == field_id)
        .ok_or(AppError::NotFound)?;
    let renamed = core::rename_field_definition(&definition, name)?;
    repo.update_field_definition(&renamed).await?;
    Ok(renamed)
}

/// Adds a new project-local (custom) relationship kind.
pub async fn add_relationship_kind(
    repo: &dyn ProjectRepository,
    ids: &dyn IdGen,
    role: Option<Role>,
    project_id: ProjectId,
    forward_label: &str,
    reverse_label: &str,
) -> Result<RelationshipKind, AppError> {
    if !is_allowed(role, Action::ManageRelationshipKinds) {
        return Err(AppError::Forbidden);
    }
    repo.load(project_id).await?.ok_or(AppError::NotFound)?;
    let kind = core::create_relationship_kind(
        KindId::new(ids.next()),
        project_id,
        forward_label,
        reverse_label,
    )?;
    repo.insert_relationship_kind(&kind).await?;
    Ok(kind)
}
