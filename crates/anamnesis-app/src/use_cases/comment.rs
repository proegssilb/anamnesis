//! Comments on a task (`docs/DOMAIN.md` §3). Editing/deleting composes role
//! with authorship — see `crate::policy::can_edit_comment`.

use anamnesis_core::policy::Role;
use anamnesis_core::{TaskId, UserId};

use crate::entities::{self, Comment, CommentId};
use crate::error::AppError;
use crate::policy::{Action, can_edit_comment, is_allowed};
use crate::ports::{Clock, CommentRepository, IdGen};

/// Adds a comment to a task, attributed to `author`.
pub async fn add_comment(
    repo: &dyn CommentRepository,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    role: Option<Role>,
    task_id: TaskId,
    author: UserId,
    body: &str,
) -> Result<Comment, AppError> {
    if !is_allowed(role, Action::CreateComment) {
        return Err(AppError::Forbidden);
    }
    let comment = entities::create_comment(
        CommentId::new(ids.next()),
        task_id,
        author,
        body,
        clock.now(),
    )?;
    repo.insert(&comment).await?;
    Ok(comment)
}

/// Lists a task's comments.
pub async fn list_comments(
    repo: &dyn CommentRepository,
    role: Option<Role>,
    task_id: TaskId,
) -> Result<Vec<Comment>, AppError> {
    if !is_allowed(role, Action::ViewTask) {
        return Err(AppError::Forbidden);
    }
    Ok(repo.list_for_task(task_id).await?)
}

/// Edits a comment's body: allowed for its author, or a Project/System
/// Admin (`crate::policy::can_edit_comment`).
pub async fn edit_comment(
    repo: &dyn CommentRepository,
    clock: &dyn Clock,
    role: Option<Role>,
    editor: &UserId,
    id: CommentId,
    body: &str,
) -> Result<Comment, AppError> {
    let comment = repo.load(id).await?.ok_or(AppError::NotFound)?;
    if !can_edit_comment(role, &comment.author, editor) {
        return Err(AppError::Forbidden);
    }
    let edited = entities::edit_comment(&comment, body, clock.now())?;
    repo.update(&edited).await?;
    Ok(edited)
}

/// Deletes a comment: allowed for its author, or a Project/System Admin.
pub async fn delete_comment(
    repo: &dyn CommentRepository,
    role: Option<Role>,
    editor: &UserId,
    id: CommentId,
) -> Result<(), AppError> {
    let comment = repo.load(id).await?.ok_or(AppError::NotFound)?;
    if !can_edit_comment(role, &comment.author, editor) {
        return Err(AppError::Forbidden);
    }
    repo.delete(id).await?;
    Ok(())
}
