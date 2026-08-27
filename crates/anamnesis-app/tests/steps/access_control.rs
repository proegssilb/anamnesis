//! Steps for `access_control.feature`: the real permission matrix
//! (`crate::policy`), exercised through the actual Phase D use cases against
//! `domain_fakes::Fakes` -- role in, `AppError::Forbidden` (or success) out.
//!
//! Every `Given` role assignment below is written into `Fakes`' real
//! membership tables (not just a per-scenario flat value), and every `When`
//! step resolves the acting role through `MembershipQuery`
//! (`effective_role` / `effective_area_role`) rather than a shortcut --
//! which is what lets the Area-role-inheritance scenarios exercise genuine
//! resolution instead of a value the step merely echoes back.

use cucumber::{given, then, when};

use anamnesis_app::{
    AppError, MembershipQuery, add_field_definition, archive_project, create_area, create_project,
    view_area, view_project, view_task,
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
    let project_id = world.domain_project(&project_name);
    let user_id = world.user(&user);
    world
        .domain
        .set_project_role(&user_id, project_id, Role::Member);
    world.set_domain_role(&user, Some(Role::Member));
}

#[given(regex = r#"^"([^"]+)" is a Project Admin of "([^"]+)"$"#)]
async fn is_a_project_admin_of(world: &mut AppWorld, user: String, project_name: String) {
    let project_id = world.domain_project(&project_name);
    let user_id = world.user(&user);
    world
        .domain
        .set_project_role(&user_id, project_id, Role::ProjectAdmin);
    world.set_domain_role(&user, Some(Role::ProjectAdmin));
}

#[given(regex = r#"^"([^"]+)" is a System Admin$"#)]
async fn is_a_system_admin(world: &mut AppWorld, user: String) {
    let user_id = world.user(&user);
    world.domain.make_system_admin(&user_id);
    world.set_domain_role(&user, Some(Role::SystemAdmin));
}

#[given(regex = r#"^"([^"]+)" is a Project Admin of the area that contains "([^"]+)"$"#)]
async fn is_a_project_admin_of_the_area_that_contains(
    world: &mut AppWorld,
    user: String,
    project_name: String,
) {
    let area_id = world.domain_area_of(&project_name);
    let user_id = world.user(&user);
    world
        .domain
        .set_area_role(&user_id, area_id, Role::ProjectAdmin);
}

#[given(regex = r#"^"([^"]+)" is another project in the area that contains "([^"]+)"$"#)]
async fn is_another_project_in_the_area_that_contains(
    world: &mut AppWorld,
    new_project_name: String,
    existing_project_name: String,
) {
    let area_id = world.domain_area_of(&existing_project_name);
    world.domain_project_in_area(&new_project_name, area_id);
}

#[when(regex = r#"^"([^"]+)"(?: \([^)]*\))? tries to view project "([^"]+)"$"#)]
async fn tries_to_view_project(world: &mut AppWorld, user: String, project_name: String) {
    let user_id = world.user(&user);
    let project_id = world.domain_project(&project_name);
    let area_id = world.domain.project(project_id).area_id;
    let role = MembershipQuery::effective_role(&world.domain, &user_id, project_id, area_id)
        .await
        .unwrap();
    let result = view_project(&world.domain, role, project_id).await;
    world.last_domain_error = result.err();
}

#[when(regex = r#"^"([^"]+)"(?: \([^)]*\))? tries to view task "([^"]+)"$"#)]
async fn tries_to_view_task(world: &mut AppWorld, user: String, task_name: String) {
    let user_id = world.user(&user);
    let task_id = world.domain_task_id(&task_name);
    let project_id = world.domain.task(task_id).project_id;
    let area_id = world.domain.project(project_id).area_id;
    let role = MembershipQuery::effective_role(&world.domain, &user_id, project_id, area_id)
        .await
        .unwrap();
    let result = view_task(&world.domain, role, task_id).await;
    world.last_domain_error = result.err();
}

#[when(
    regex = r#"^"([^"]+)"(?: \([^)]*\))? tries to add a field definition to project "([^"]+)"$"#
)]
async fn tries_to_add_field_definition(world: &mut AppWorld, user: String, project_name: String) {
    let user_id = world.user(&user);
    let project_id = world.domain_project(&project_name);
    let area_id = world.domain.project(project_id).area_id;
    let role = MembershipQuery::effective_role(&world.domain, &user_id, project_id, area_id)
        .await
        .unwrap();
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
    let user_id = world.user(&user);
    let project_id = world.domain_project(&project_name);
    let area_id = world.domain.project(project_id).area_id;
    let role = MembershipQuery::effective_role(&world.domain, &user_id, project_id, area_id)
        .await
        .unwrap();
    let result =
        archive_project(&world.domain, &world.clock, &world.domain, role, project_id).await;
    world.last_domain_error = result.err();
}

#[when(regex = r#"^"([^"]+)"(?: \([^)]*\))? tries to create an area named "([^"]+)"$"#)]
async fn tries_to_create_area(world: &mut AppWorld, user: String, area_name: String) {
    // Creating an Area has no existing Area to resolve a role *in* -- the
    // only membership a caller could ever legitimately hold here is System
    // Admin (see `crate::policy`'s module doc comment).
    let user_id = world.user(&user);
    let role = if world.domain.is_system_admin(&user_id).await.unwrap() {
        Some(Role::SystemAdmin)
    } else {
        None
    };
    let result = create_area(
        &world.domain,
        &world.ids,
        &world.clock,
        &world.domain,
        role,
        &area_name,
        "",
        0,
    )
    .await;
    world.last_domain_error = result.err();
}

#[when(regex = r#"^"([^"]+)"(?: \([^)]*\))? tries to view the area that contains "([^"]+)"$"#)]
async fn tries_to_view_the_area_that_contains(
    world: &mut AppWorld,
    user: String,
    project_name: String,
) {
    let user_id = world.user(&user);
    let area_id = world.domain_area_of(&project_name);
    let role = MembershipQuery::effective_area_role(&world.domain, &user_id, area_id)
        .await
        .unwrap();
    let result = view_area(&world.domain, role, area_id).await;
    world.last_domain_error = result.err();
}

#[when(
    regex = r#"^"([^"]+)"(?: \([^)]*\))? tries to create a project named "([^"]+)" in the area that contains "([^"]+)"$"#
)]
async fn tries_to_create_a_project_in_the_area_that_contains(
    world: &mut AppWorld,
    user: String,
    new_project_name: String,
    existing_project_name: String,
) {
    let user_id = world.user(&user);
    let area_id = world.domain_area_of(&existing_project_name);
    let role = MembershipQuery::effective_area_role(&world.domain, &user_id, area_id)
        .await
        .unwrap();
    let result = create_project(
        &world.domain,
        &world.ids,
        &world.clock,
        &world.domain,
        role,
        area_id,
        &new_project_name,
        "",
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
