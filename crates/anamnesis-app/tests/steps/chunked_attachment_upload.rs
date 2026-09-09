//! Steps for `chunked_attachment_upload.feature`: the multi-request upload
//! flow (`anamnesis_app::{begin_file_upload, upload_file_part,
//! complete_file_upload, abort_file_upload}`), exercised against
//! `domain_fakes::Fakes` exactly like `collaboration.rs`'s single-request
//! attachment steps, but kept in its own file per the flow's own separate
//! use cases.

use cucumber::{then, when};

use anamnesis_app::{
    AppError, AttachmentKind, NewUpload, abort_file_upload, begin_file_upload,
    complete_file_upload, upload_file_part,
};

use crate::support::byte_stream;

use super::AppWorld;

#[when(
    regex = r#"^"([^"]+)"(?: \([^)]*\))? (?:begins|tries to begin) uploading a file "([^"]+)" to task "([^"]+)"$"#
)]
async fn begins_uploading(world: &mut AppWorld, user: String, filename: String, task_name: String) {
    let role = world.domain_role(&user);
    let task_id = world.domain_task_id(&task_name);
    let created_by = world.user(&user);
    match begin_file_upload(
        &world.domain,
        &world.domain,
        &world.ids,
        &world.clock,
        role,
        NewUpload {
            task_id,
            created_by,
            filename: &filename,
            mime: "application/octet-stream",
        },
    )
    .await
    {
        Ok(upload) => {
            world.set_domain_upload(&filename, upload.id);
            world.last_domain_error = None;
        }
        Err(err) => world.last_domain_error = Some(err),
    }
}

#[when(regex = r#"^"([^"]+)" uploads part (\d+) of "([^"]+)" with (\d+) bytes$"#)]
async fn uploads_part(
    world: &mut AppWorld,
    user: String,
    part_number: u32,
    filename: String,
    byte_count: usize,
) {
    let role = world.domain_role(&user);
    let upload_id = world.domain_upload_id(&filename);
    let data = byte_stream(vec![0u8; byte_count]);
    upload_file_part(
        &world.domain,
        &world.domain,
        role,
        u64::MAX,
        upload_id,
        part_number,
        data,
    )
    .await
    .expect("scenario setup: uploading a part must succeed");
}

#[when(
    regex = r#"^"([^"]+)" tries to upload part (\d+) of "([^"]+)" with (\d+) bytes against a (\d+) byte attachment cap$"#
)]
async fn tries_to_upload_part_over_cap(
    world: &mut AppWorld,
    user: String,
    part_number: u32,
    filename: String,
    byte_count: usize,
    cap: u64,
) {
    let role = world.domain_role(&user);
    let upload_id = world.domain_upload_id(&filename);
    let data = byte_stream(vec![0u8; byte_count]);
    let result = upload_file_part(
        &world.domain,
        &world.domain,
        role,
        cap,
        upload_id,
        part_number,
        data,
    )
    .await;
    world.last_domain_error = result.err();
}

#[when(regex = r#"^"([^"]+)" completes uploading "([^"]+)"$"#)]
async fn completes_uploading(world: &mut AppWorld, user: String, filename: String) {
    let role = world.domain_role(&user);
    let upload_id = world.domain_upload_id(&filename);
    let attachment = complete_file_upload(
        &world.domain,
        &world.domain,
        &world.domain,
        &world.ids,
        &world.clock,
        role,
        upload_id,
    )
    .await
    .expect("scenario setup: completing the upload must succeed");
    if let AttachmentKind::File { blob_key, .. } = &attachment.kind {
        world.set_domain_blob_key(&filename, blob_key.clone());
    }
    world.set_domain_attachment(&filename, attachment.id);
}

#[when(regex = r#"^"([^"]+)" aborts uploading "([^"]+)"$"#)]
async fn aborts_uploading(world: &mut AppWorld, user: String, filename: String) {
    let role = world.domain_role(&user);
    let upload_id = world.domain_upload_id(&filename);
    abort_file_upload(&world.domain, &world.domain, role, upload_id)
        .await
        .expect("scenario setup: aborting the upload must succeed");
}

#[then(expr = "the upload is rejected as too large")]
async fn the_upload_is_rejected_as_too_large(world: &mut AppWorld) {
    assert!(
        matches!(world.last_domain_error, Some(AppError::Invalid(_))),
        "expected AppError::Invalid, got {:?}",
        world.last_domain_error
    );
}
