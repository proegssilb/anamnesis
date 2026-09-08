//! Steps for `archive_sweep.feature`: "archive all", exercised through the
//! real `archive_done_tasks` use case against `domain_fakes::Fakes`. The
//! `"X" is archived`/`"X" is not archived` assertions this feature relies on
//! are registered once, in `task_lifecycle.rs` -- shared by phrase, not by
//! source file, exactly as `placement.rs`'s module doc comment already
//! explains for its own borrowed steps.

use cucumber::when;

use anamnesis_app::archive_done_tasks;

use super::AppWorld;

#[when(regex = r#"^"([^"]+)"(?: \([^)]*\))? archives all done work$"#)]
async fn archives_all_done_work(world: &mut AppWorld, user: String) {
    let role = world.domain_role(&user);
    let result = archive_done_tasks(
        &world.domain,
        &world.domain,
        &world.domain,
        &world.clock,
        &world.domain,
        role,
    )
    .await;
    world.last_domain_error = result.err();
}
