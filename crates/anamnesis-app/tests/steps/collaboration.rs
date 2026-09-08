//! Steps for `collaboration.feature`: comments (authorship-composed
//! edit/delete permission) and attachments (link vs. file, blob cleanup on
//! delete) -- exercised through the real `add_comment`/`edit_comment`/
//! `add_link_attachment`/`add_file_attachment`/`delete_attachment` use cases
//! against `domain_fakes::Fakes`.

use cucumber::{then, when};

use anamnesis_app::{
    AttachmentRepository, BlobStore, CommentRepository, add_comment, add_file_attachment,
    add_link_attachment, delete_attachment, edit_comment,
};

use super::AppWorld;

#[when(regex = r#"^"([^"]+)" comments "([^"]+)" on task "([^"]+)"$"#)]
async fn comments_on_task(world: &mut AppWorld, author: String, body: String, task_name: String) {
    let role = world.domain_role(&author);
    let task_id = world.domain_task_id(&task_name);
    let author_id = world.user(&author);
    let comment = add_comment(
        &world.domain,
        &world.ids,
        &world.clock,
        role,
        task_id,
        author_id,
        &body,
    )
    .await
    .expect("scenario setup: adding the comment must succeed");
    world.set_domain_comment(&author, comment.id);
}

#[when(regex = r#"^"([^"]+)" edits her comment on task "([^"]+)" to "([^"]+)"$"#)]
async fn edits_her_comment(world: &mut AppWorld, user: String, _task_name: String, body: String) {
    let role = world.domain_role(&user);
    let editor_id = world.user(&user);
    let comment_id = world.domain_comment_id(&user);
    let result = edit_comment(
        &world.domain,
        &world.clock,
        role,
        &editor_id,
        comment_id,
        &body,
    )
    .await;
    world.last_domain_error = result.err();
}

#[when(regex = r#"^"([^"]+)" tries to edit ([A-Za-z]+)'s comment on task "([^"]+)" to "([^"]+)"$"#)]
async fn tries_to_edit_someone_elses_comment(
    world: &mut AppWorld,
    editor: String,
    author: String,
    _task_name: String,
    body: String,
) {
    let role = world.domain_role(&editor);
    let editor_id = world.user(&editor);
    let comment_id = world.domain_comment_id(&author);
    let result = edit_comment(
        &world.domain,
        &world.clock,
        role,
        &editor_id,
        comment_id,
        &body,
    )
    .await;
    world.last_domain_error = result.err();
}

#[then(regex = r#"^task "([^"]+)" has (\d+) comments?$"#)]
async fn task_has_n_comments(world: &mut AppWorld, task_name: String, count: usize) {
    let task_id = world.domain_task_id(&task_name);
    let comments = CommentRepository::list_for_task(&world.domain, task_id)
        .await
        .unwrap();
    assert_eq!(comments.len(), count);
}

#[then(regex = r#"^the comment on task "([^"]+)" reads "([^"]+)"$"#)]
async fn the_comment_on_task_reads(world: &mut AppWorld, task_name: String, body: String) {
    let task_id = world.domain_task_id(&task_name);
    let comments = CommentRepository::list_for_task(&world.domain, task_id)
        .await
        .unwrap();
    assert_eq!(comments.len(), 1, "expected exactly one comment");
    assert_eq!(comments[0].body, body);
}

#[when(regex = r#"^"([^"]+)" attaches the link "([^"]+)" to task "([^"]+)"$"#)]
async fn attaches_the_link(world: &mut AppWorld, user: String, url: String, task_name: String) {
    let role = world.domain_role(&user);
    let task_id = world.domain_task_id(&task_name);
    let attachment =
        add_link_attachment(&world.domain, &world.ids, &world.clock, role, task_id, &url)
            .await
            .expect("scenario setup: attaching the link must succeed");
    world.set_domain_attachment(&url, attachment.id);
}

#[when(regex = r#"^"([^"]+)" attaches a file "([^"]+)" to task "([^"]+)"$"#)]
async fn attaches_a_file(world: &mut AppWorld, user: String, filename: String, task_name: String) {
    let role = world.domain_role(&user);
    let task_id = world.domain_task_id(&task_name);
    let attachment = add_file_attachment(
        &world.domain,
        &world.domain,
        &world.ids,
        &world.clock,
        role,
        task_id,
        &filename,
        "application/pdf",
        vec![1, 2, 3, 4],
    )
    .await
    .expect("scenario setup: attaching the file must succeed");
    if let anamnesis_app::AttachmentKind::File { blob_key, .. } = &attachment.kind {
        world.set_domain_blob_key(&filename, blob_key.clone());
    }
    world.set_domain_attachment(&filename, attachment.id);
}

#[then(regex = r#"^task "([^"]+)" has (\d+) attachments?$"#)]
async fn task_has_n_attachments(world: &mut AppWorld, task_name: String, count: usize) {
    let task_id = world.domain_task_id(&task_name);
    let attachments = AttachmentRepository::list_for_task(&world.domain, task_id)
        .await
        .unwrap();
    assert_eq!(attachments.len(), count);
}

#[then(regex = r#"^the file "([^"]+)" is stored in the blob store$"#)]
async fn the_file_is_stored(world: &mut AppWorld, filename: String) {
    let blob_key = world.domain_blob_key(&filename);
    let stored = BlobStore::get(&world.domain, &blob_key).await.unwrap();
    assert!(
        stored.is_some(),
        "expected {filename:?}'s blob to be stored"
    );
}

#[then(regex = r#"^the file "([^"]+)" is no longer stored in the blob store$"#)]
async fn the_file_is_no_longer_stored(world: &mut AppWorld, filename: String) {
    let blob_key = world.domain_blob_key(&filename);
    let stored = BlobStore::get(&world.domain, &blob_key).await.unwrap();
    assert!(stored.is_none(), "expected {filename:?}'s blob to be gone");
}

#[when(regex = r#"^"([^"]+)" deletes the file attachment "([^"]+)" from task "([^"]+)"$"#)]
async fn deletes_the_file_attachment(
    world: &mut AppWorld,
    user: String,
    filename: String,
    _task_name: String,
) {
    let role = world.domain_role(&user);
    let attachment_id = world.domain_attachment_id(&filename);
    let result = delete_attachment(&world.domain, &world.domain, role, attachment_id).await;
    world.last_domain_error = result.err();
}
