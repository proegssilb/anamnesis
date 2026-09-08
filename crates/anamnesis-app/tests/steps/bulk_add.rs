//! Steps for `bulk_add.feature` (issue #34): bulk-creating areas, projects,
//! and tasks straight through `anamnesis_app::{bulk_create_areas,
//! bulk_create_projects, bulk_create_tasks}` against `domain_fakes::Fakes`
//! -- the app-layer half of the feature. The web layer's own
//! newline-per-title parsing and HTTP-level behaviour on top of these same
//! use cases is covered separately, by `anamnesis-web`'s `tests/bulk_add.rs`.

use cucumber::{then, when};

use anamnesis_app::{
    AreaRepository, MembershipQuery, ProjectRepository, TaskRepository, bulk_create_areas,
    bulk_create_projects, bulk_create_tasks,
};
use anamnesis_core::policy::Role;

use super::AppWorld;

/// Splits a Gherkin-quoted, comma-separated title list like
/// `"Home", "Health"` into its titles, in the order written.
fn parse_quoted_titles(raw: &str) -> Vec<String> {
    raw.split(", ")
        .map(|s| s.trim_matches('"').to_string())
        .collect()
}

#[when(regex = r#"^"([^"]+)" bulk-creates areas: (.+)$"#)]
async fn bulk_creates_areas(world: &mut AppWorld, user: String, raw_titles: String) {
    let user_id = world.user(&user);
    let admin = MembershipQuery::is_system_admin(&world.domain, &user_id)
        .await
        .unwrap();
    let role = admin.then_some(Role::SystemAdmin);
    let titles = parse_quoted_titles(&raw_titles);
    let title_refs: Vec<&str> = titles.iter().map(String::as_str).collect();

    let outcome = bulk_create_areas(
        &world.domain,
        &world.ids,
        &world.clock,
        &world.domain,
        role,
        &title_refs,
    )
    .await
    .expect("scenario setup: bulk-creating areas must succeed");
    world.last_bulk_rejected = outcome.failures.len();
}

#[when(
    regex = r#"^"([^"]+)" bulk-creates areas including one title over 200 characters, plus "([^"]+)" and "([^"]+)"$"#
)]
async fn bulk_creates_areas_with_a_rejected_title(
    world: &mut AppWorld,
    user: String,
    first: String,
    second: String,
) {
    let user_id = world.user(&user);
    let admin = MembershipQuery::is_system_admin(&world.domain, &user_id)
        .await
        .unwrap();
    let role = admin.then_some(Role::SystemAdmin);
    let too_long = "x".repeat(201);
    let titles = [too_long.as_str(), first.as_str(), second.as_str()];

    let outcome = bulk_create_areas(
        &world.domain,
        &world.ids,
        &world.clock,
        &world.domain,
        role,
        &titles,
    )
    .await
    .expect("scenario setup: bulk-creating areas must succeed even with a rejected title");
    world.last_bulk_rejected = outcome.failures.len();
}

#[when(regex = r#"^"([^"]+)" bulk-creates projects in area "([^"]+)": (.+)$"#)]
async fn bulk_creates_projects(
    world: &mut AppWorld,
    user: String,
    area_name: String,
    raw_titles: String,
) {
    let user_id = world.user(&user);
    let area_id = world.domain_area(&area_name);
    let role = MembershipQuery::effective_area_role(&world.domain, &user_id, area_id)
        .await
        .unwrap();
    let titles = parse_quoted_titles(&raw_titles);
    let title_refs: Vec<&str> = titles.iter().map(String::as_str).collect();

    let outcome = bulk_create_projects(
        &world.domain,
        &world.ids,
        &world.clock,
        &world.domain,
        role,
        area_id,
        &title_refs,
    )
    .await
    .expect("scenario setup: bulk-creating projects must succeed");
    world.last_bulk_rejected = outcome.failures.len();
}

#[when(regex = r#"^"([^"]+)" bulk-creates tasks in project "([^"]+)": (.+)$"#)]
async fn bulk_creates_tasks(
    world: &mut AppWorld,
    user: String,
    project_name: String,
    raw_titles: String,
) {
    let user_id = world.user(&user);
    let project_id = world.domain_project(&project_name);
    let area_id = world.domain.project(project_id).area_id;
    let role = MembershipQuery::effective_role(&world.domain, &user_id, project_id, area_id)
        .await
        .unwrap();
    let titles = parse_quoted_titles(&raw_titles);
    let title_refs: Vec<&str> = titles.iter().map(String::as_str).collect();

    let outcome = bulk_create_tasks(
        &world.domain,
        &world.ids,
        &world.clock,
        &world.domain,
        role,
        project_id,
        &title_refs,
    )
    .await
    .expect("scenario setup: bulk-creating tasks must succeed");
    world.last_bulk_rejected = outcome.failures.len();
}

#[then(regex = r#"^areas (.+?) all exist$"#)]
async fn areas_all_exist(world: &mut AppWorld, raw_titles: String) {
    let titles = parse_quoted_titles(&raw_titles);
    let existing: Vec<String> = AreaRepository::list(&world.domain)
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.title.as_str().to_string())
        .collect();
    for title in &titles {
        assert!(
            existing.contains(title),
            "expected area {title:?} to exist, found {existing:?}"
        );
    }
}

#[then(regex = r#"^projects (.+?) all exist in area "([^"]+)"$"#)]
async fn projects_all_exist_in_area(world: &mut AppWorld, raw_titles: String, area_name: String) {
    let titles = parse_quoted_titles(&raw_titles);
    let area_id = world.domain_area(&area_name);
    let existing: Vec<String> = ProjectRepository::list_by_area(&world.domain, area_id)
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.title.as_str().to_string())
        .collect();
    for title in &titles {
        assert!(
            existing.contains(title),
            "expected project {title:?} to exist in area {area_name:?}, found {existing:?}"
        );
    }
}

#[then(regex = r#"^tasks (.+?) all exist in project "([^"]+)"$"#)]
async fn tasks_all_exist_in_project(
    world: &mut AppWorld,
    raw_titles: String,
    project_name: String,
) {
    let titles = parse_quoted_titles(&raw_titles);
    let project_id = world.domain_project(&project_name);
    let existing: Vec<String> = TaskRepository::list_by_project(&world.domain, project_id)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.title.as_str().to_string())
        .collect();
    for title in &titles {
        assert!(
            existing.contains(title),
            "expected task {title:?} to exist in project {project_name:?}, found {existing:?}"
        );
    }
}

#[then(regex = r#"^the bulk create rejected exactly (\d+) titles?$"#)]
async fn the_bulk_create_rejected_exactly(world: &mut AppWorld, count: usize) {
    assert_eq!(
        world.last_bulk_rejected, count,
        "expected exactly {count} rejected title(s)"
    );
}
