//! Steps for `access_control.feature`: the real permission matrix
//! (`crate::policy`), exercised through the actual Phase D use cases against
//! `domain_fakes::Fakes` -- role in, `AppError::Forbidden` (or success) out.

use cucumber::{given, then, when};

use anamnesis_app::{
    AppError, add_field_definition, archive_project, create_area, view_project, view_task,
};
use anamnesis_core::FieldKind;
use anamnesis_core::policy::Role;

use super::AppWorld;

#[given(expr = "a task {string} below the horizon in project {string}")]
async fn a_task_below_the_horizon_in_project(
    world: &mut AppWorld,
    task_name: String,
    project_name: String,
) {
    world.domain_task(&task_name, &project_name);
}

#[given(regex = r#"^"([^"]+)" is a Member of "([^"]+)"$"#)]
async fn is_a_member_of(world: &mut AppWorld, user: String, project_name: String) {
    world.domain_project(&project_name);
    world.set_domain_role(&user, Some(Role::Member));
}

#[given(regex = r#"^"([^"]+)" is a Project Admin of "([^"]+)"$"#)]
async fn is_a_project_admin_of(world: &mut AppWorld, user: String, project_name: String) {
    world.domain_project(&project_name);
    world.set_domain_role(&user, Some(Role::ProjectAdmin));
}

#[given(regex = r#"^"([^"]+)" is a System Admin$"#)]
async fn is_a_system_admin(world: &mut AppWorld, user: String) {
    world.set_domain_role(&user, Some(Role::SystemAdmin));
}

#[when(regex = r#"^"([^"]+)"(?: \([^)]*\))? tries to view project "([^"]+)"$"#)]
async fn tries_to_view_project(world: &mut AppWorld, user: String, project_name: String) {
    let role = world.domain_role(&user);
    let project_id = world.domain_project(&project_name);
    let result = view_project(&world.domain, role, project_id).await;
    world.last_domain_error = result.err();
}

#[when(regex = r#"^"([^"]+)"(?: \([^)]*\))? tries to view task "([^"]+)"$"#)]
async fn tries_to_view_task(world: &mut AppWorld, user: String, task_name: String) {
    let role = world.domain_role(&user);
    let task_id = world.domain_task_id(&task_name);
    let result = view_task(&world.domain, role, task_id).await;
    world.last_domain_error = result.err();
}

#[when(
    regex = r#"^"([^"]+)"(?: \([^)]*\))? tries to add a field definition to project "([^"]+)"$"#
)]
async fn tries_to_add_field_definition(world: &mut AppWorld, user: String, project_name: String) {
    let role = world.domain_role(&user);
    let project_id = world.domain_project(&project_name);
    let result = add_field_definition(
        &world.domain,
        &world.ids,
        role,
        project_id,
        "Priority",
        FieldKind::Number,
        0,
        true,
    )
    .await;
    world.last_domain_error = result.err();
}

#[when(regex = r#"^"([^"]+)"(?: \([^)]*\))? tries to archive project "([^"]+)"$"#)]
async fn tries_to_archive_project(world: &mut AppWorld, user: String, project_name: String) {
    let role = world.domain_role(&user);
    let project_id = world.domain_project(&project_name);
    let result = archive_project(&world.domain, &world.clock, role, project_id).await;
    world.last_domain_error = result.err();
}

#[when(regex = r#"^"([^"]+)"(?: \([^)]*\))? tries to create an area named "([^"]+)"$"#)]
async fn tries_to_create_area(world: &mut AppWorld, user: String, area_name: String) {
    let role = world.domain_role(&user);
    let result = create_area(
        &world.domain,
        &world.ids,
        &world.clock,
        role,
        &area_name,
        "",
        0,
    )
    .await;
    world.last_domain_error = result.err();
}

#[then(expr = "access is granted")]
async fn access_is_granted(world: &mut AppWorld) {
    assert!(
        world.last_domain_error.is_none(),
        "expected access to be granted, got {:?}",
        world.last_domain_error
    );
}

#[then(expr = "access is refused")]
async fn access_is_refused(world: &mut AppWorld) {
    assert!(
        matches!(world.last_domain_error, Some(AppError::Forbidden)),
        "expected access to be refused, got {:?}",
        world.last_domain_error
    );
}
