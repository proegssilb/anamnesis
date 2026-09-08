//! Steps for `project_lifecycle.feature`: creating a project, the
//! active-project-limit guard on activating one, and archiving/unarchiving
//! -- exercised through the real `create_project`/`transition_project_status`/
//! `archive_project`/`unarchive_project` use cases against
//! `domain_fakes::Fakes`.

use cucumber::{given, then, when};

use anamnesis_app::{AppError, MembershipQuery, create_project, transition_project_status};
use anamnesis_core::policy::Role;
use anamnesis_core::{DomainError, ProjectStatus};

use super::AppWorld;

#[given(regex = r#"^"([^"]+)" is a Project Admin of area "([^"]+)"$"#)]
async fn is_a_project_admin_of_area(world: &mut AppWorld, user: String, area_name: String) {
    let area_id = world.domain_area(&area_name);
    let user_id = world.user(&user);
    world
        .domain
        .set_area_role(&user_id, area_id, Role::ProjectAdmin);
}

#[when(regex = r#"^"([^"]+)" creates a project named "([^"]+)" in area "([^"]+)"$"#)]
async fn creates_a_project_in_area(
    world: &mut AppWorld,
    user: String,
    project_name: String,
    area_name: String,
) {
    let user_id = world.user(&user);
    let area_id = world.domain_area(&area_name);
    let role = MembershipQuery::effective_area_role(&world.domain, &user_id, area_id)
        .await
        .unwrap();
    let project = create_project(
        &world.domain,
        &world.ids,
        &world.clock,
        &world.domain,
        role,
        area_id,
        &project_name,
        "",
    )
    .await
    .expect("scenario setup: creating the project must succeed");
    world.register_domain_project(&project_name, project.id);
}

#[then(regex = r#"^project "([^"]+)" is Pending$"#)]
async fn project_is_pending(world: &mut AppWorld, project_name: String) {
    let project_id = world.domain_project(&project_name);
    assert_eq!(
        world.domain.project(project_id).status,
        ProjectStatus::Pending
    );
}

#[when(
    regex = r#"^"([^"]+)" tries to activate project "([^"]+)" against an active-project limit of (\d+)$"#
)]
async fn tries_to_activate_project(
    world: &mut AppWorld,
    user: String,
    project_name: String,
    limit: u32,
) {
    let user_id = world.user(&user);
    let project_id = world.domain_project(&project_name);
    let area_id = world.domain.project(project_id).area_id;
    let role = MembershipQuery::effective_role(&world.domain, &user_id, project_id, area_id)
        .await
        .unwrap();
    let result = transition_project_status(
        &world.domain,
        &world.clock,
        role,
        project_id,
        ProjectStatus::Active,
        limit,
    )
    .await;
    world.last_domain_error = result.err();
}

#[then(expr = "the activation is refused because the active-project limit is reached")]
async fn activation_refused_limit(world: &mut AppWorld) {
    assert!(
        matches!(
            world.last_domain_error,
            Some(AppError::Rule(DomainError::ActiveProjectLimitExceeded))
        ),
        "expected an active-project-limit refusal, got {:?}",
        world.last_domain_error
    );
}

#[when(regex = r#"^"([^"]+)" tries to unarchive project "([^"]+)"$"#)]
async fn tries_to_unarchive_project(world: &mut AppWorld, user: String, project_name: String) {
    let user_id = world.user(&user);
    let project_id = world.domain_project(&project_name);
    let area_id = world.domain.project(project_id).area_id;
    let role = MembershipQuery::effective_role(&world.domain, &user_id, project_id, area_id)
        .await
        .unwrap();
    let result = anamnesis_app::unarchive_project(
        &world.domain,
        &world.clock,
        &world.domain,
        role,
        project_id,
    )
    .await;
    world.last_domain_error = result.err();
}

#[then(regex = r#"^project "([^"]+)" is archived$"#)]
async fn project_is_archived(world: &mut AppWorld, project_name: String) {
    let project_id = world.domain_project(&project_name);
    assert!(
        world.domain.project(project_id).archived_at.is_some(),
        "expected project {project_name:?} to be archived"
    );
}

#[then(regex = r#"^project "([^"]+)" is not archived$"#)]
async fn project_is_not_archived(world: &mut AppWorld, project_name: String) {
    let project_id = world.domain_project(&project_name);
    assert!(
        world.domain.project(project_id).archived_at.is_none(),
        "expected project {project_name:?} not to be archived"
    );
}
