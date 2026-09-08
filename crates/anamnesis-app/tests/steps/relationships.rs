//! Steps for `relationships.feature`: linking tasks with a built-in kind
//! (cross-project) or a project-local custom kind (same-project only),
//! self-relationship rejection, and deletion -- exercised through the real
//! `create_relationship`/`delete_relationship`/`add_relationship_kind` use
//! cases against `domain_fakes::Fakes`.

use cucumber::{given, then, when};

use anamnesis_app::{
    AppError, RelationshipRepository, add_relationship_kind, create_relationship,
    delete_relationship,
};
use anamnesis_core::policy::Role;
use anamnesis_core::{DomainError, KindId};

use super::AppWorld;

/// Creates a relationship edge, resolving `from`/`to`'s owning projects from
/// the store (`create_relationship` takes them explicitly -- edges live
/// outside any single project, per `docs/DOMAIN.md` SS3).
async fn link_with_role(
    world: &mut AppWorld,
    role: Option<Role>,
    from: &str,
    to: &str,
    kind_id: KindId,
) -> Result<anamnesis_core::Relationship, AppError> {
    let from_id = world.domain_task_id(from);
    let from_project = world.domain.task(from_id).project_id;
    let to_id = world.domain_task_id(to);
    let to_project = world.domain.task(to_id).project_id;
    create_relationship(
        &world.domain,
        &world.domain,
        &world.ids,
        &world.clock,
        role,
        from_id,
        from_project,
        to_id,
        to_project,
        kind_id,
    )
    .await
}

#[given(regex = r#"^"([^"]+)" is already blocking "([^"]+)"$"#)]
async fn given_blocks(world: &mut AppWorld, from: String, to: String) {
    // Scenario setup, not the behaviour under test: run as a System Admin so
    // the permission gate (exercised elsewhere in this suite) never gets in
    // the way of establishing the edge a `When`/`Then` step needs.
    let relationship = link_with_role(
        world,
        Some(Role::SystemAdmin),
        &from,
        &to,
        KindId::BUILTIN_BLOCKS,
    )
    .await
    .expect("scenario setup: creating the blocks edge must succeed");
    world.set_domain_relationship(&from, &to, relationship.id);
}

#[when(regex = r#"^"([^"]+)" (?:tries to )?links? "([^"]+)" as blocking "([^"]+)"$"#)]
async fn links_as_blocking(world: &mut AppWorld, actor: String, from: String, to: String) {
    let role = world.domain_role(&actor);
    let result = link_with_role(world, role, &from, &to, KindId::BUILTIN_BLOCKS).await;
    match result {
        Ok(relationship) => {
            world.set_domain_relationship(&from, &to, relationship.id);
            world.last_domain_error = None;
        }
        Err(err) => world.last_domain_error = Some(err),
    }
}

#[then(regex = r#"^"([^"]+)" is blocked by "([^"]+)"$"#)]
async fn is_blocked_by(world: &mut AppWorld, to: String, from: String) {
    let to_id = world.domain_task_id(&to);
    let from_id = world.domain_task_id(&from);
    let relationships = RelationshipRepository::list_for_task(&world.domain, to_id)
        .await
        .unwrap();
    assert!(
        relationships.iter().any(|r| r.from_task_id == from_id
            && r.to_task_id == to_id
            && r.kind_id == KindId::BUILTIN_BLOCKS),
        "expected {to:?} to be blocked by {from:?}"
    );
}

#[then(regex = r#"^"([^"]+)" is not blocked by "([^"]+)"$"#)]
async fn is_not_blocked_by(world: &mut AppWorld, to: String, from: String) {
    let to_id = world.domain_task_id(&to);
    let from_id = world.domain_task_id(&from);
    let relationships = RelationshipRepository::list_for_task(&world.domain, to_id)
        .await
        .unwrap();
    assert!(
        !relationships.iter().any(|r| r.from_task_id == from_id
            && r.to_task_id == to_id
            && r.kind_id == KindId::BUILTIN_BLOCKS),
        "expected {to:?} not to be blocked by {from:?}"
    );
}

#[given(regex = r#"^"([^"]+)" is a custom relationship kind of project "([^"]+)"$"#)]
async fn is_a_custom_relationship_kind_of(
    world: &mut AppWorld,
    label: String,
    project_name: String,
) {
    let project_id = world.domain_project(&project_name);
    let kind = add_relationship_kind(
        &world.domain,
        &world.ids,
        Some(Role::ProjectAdmin),
        project_id,
        &label,
        &label,
    )
    .await
    .expect("scenario setup: creating the custom kind must succeed");
    world.set_domain_kind(&label, kind.id);
}

#[when(regex = r#"^"([^"]+)" (?:tries to )?links? "([^"]+)" to "([^"]+)" using "([^"]+)"$"#)]
async fn links_using_kind(
    world: &mut AppWorld,
    actor: String,
    from: String,
    to: String,
    kind_label: String,
) {
    let role = world.domain_role(&actor);
    let kind_id = world.domain_kind_id(&kind_label);
    let result = link_with_role(world, role, &from, &to, kind_id).await;
    match result {
        Ok(relationship) => {
            world.set_domain_relationship(&from, &to, relationship.id);
            world.last_domain_error = None;
        }
        Err(err) => world.last_domain_error = Some(err),
    }
}

#[then(expr = "the link is refused because a custom kind may only be used within its own project")]
async fn link_refused_kind_not_allowed(world: &mut AppWorld) {
    assert!(
        matches!(
            world.last_domain_error,
            Some(AppError::Rule(DomainError::RelationshipKindNotAllowed))
        ),
        "expected a custom-kind-not-allowed refusal, got {:?}",
        world.last_domain_error
    );
}

#[then(expr = "the link is refused because a task cannot relate to itself")]
async fn link_refused_self_relationship(world: &mut AppWorld) {
    assert!(
        matches!(
            world.last_domain_error,
            Some(AppError::Rule(DomainError::SelfRelationship))
        ),
        "expected a self-relationship refusal, got {:?}",
        world.last_domain_error
    );
}

#[when(regex = r#"^"([^"]+)" deletes the link between "([^"]+)" and "([^"]+)"$"#)]
async fn deletes_the_link_between(world: &mut AppWorld, user: String, from: String, to: String) {
    let role = world.domain_role(&user);
    let relationship_id = world.domain_relationship_id(&from, &to);
    let result = delete_relationship(&world.domain, role, relationship_id).await;
    world.last_domain_error = result.err();
}
