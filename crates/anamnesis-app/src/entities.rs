//! `Comment` and `Attachment`: named as entities in `docs/DOMAIN.md` §3, but
//! never given a pure-core representation in Phases A-C, whose scope tables
//! list only Areas, Projects, Tasks, placement, containment, fields,
//! relationships, kinds, columns and tangles. Both are flat, append-heavy
//! records with no lifecycle beyond "exists" / "edited" / "deleted" and no
//! rule that needs `anamnesis-core`'s pure-transition machinery — so rather
//! than extend an already-complete, already-committed crate for two trivial
//! types, they are defined here, at the layer that actually needs them to
//! shape `CommentRepository`/`AttachmentRepository` (`docs/DOMAIN.md` §7).
//!
//! Validation is minimal and lives in the constructors below (`AppError`,
//! not `DomainError` — see `crate::error`): a comment's body and an
//! attachment's identifying fields must be non-empty once trimmed. Neither
//! goes through `anamnesis_core::Title` (its 200-character cap suits a task
//! title, not a comment body or a filename).

use serde::{Deserialize, Serialize};

use anamnesis_core::{TaskId, Timestamp, UserId};
use uuid::Uuid;

use crate::error::AppError;

macro_rules! app_id {
    ($name:ident) => {
        #[doc = concat!("A `", stringify!($name), "`, a `Uuid` wrapped for type-level distinction.")]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[doc = concat!("Wraps an externally supplied `Uuid` as a `", stringify!($name), "`.")]
            pub fn new(id: Uuid) -> Self {
                Self(id)
            }

            /// The underlying `Uuid`.
            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

app_id!(CommentId);
app_id!(AttachmentId);

/// A remark on a task, attributed to its author.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: CommentId,
    pub task_id: TaskId,
    pub author: UserId,
    pub body: String,
    pub created_at: Timestamp,
    pub edited_at: Option<Timestamp>,
}

/// Creates a new comment. Rejects a blank (post-trim) body.
pub fn create_comment(
    id: CommentId,
    task_id: TaskId,
    author: UserId,
    body: impl AsRef<str>,
    now: Timestamp,
) -> Result<Comment, AppError> {
    let body = non_blank(body, "comment body")?;
    Ok(Comment {
        id,
        task_id,
        author,
        body,
        created_at: now,
        edited_at: None,
    })
}

/// Replaces a comment's body, stamping `edited_at`.
pub fn edit_comment(
    comment: &Comment,
    body: impl AsRef<str>,
    now: Timestamp,
) -> Result<Comment, AppError> {
    let body = non_blank(body, "comment body")?;
    Ok(Comment {
        body,
        edited_at: Some(now),
        ..comment.clone()
    })
}

/// What an [`Attachment`] points at: an external link, or a file held in a
/// [`crate::ports::BlobStore`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentKind {
    Link {
        url: String,
    },
    File {
        blob_key: String,
        filename: String,
        mime: String,
        size: u64,
    },
}

/// A file or link attached to a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: AttachmentId,
    pub task_id: TaskId,
    pub kind: AttachmentKind,
    pub created_at: Timestamp,
}

/// Attaches a link to a task. Rejects a blank (post-trim) URL.
pub fn attach_link(
    id: AttachmentId,
    task_id: TaskId,
    url: impl AsRef<str>,
    now: Timestamp,
) -> Result<Attachment, AppError> {
    let url = non_blank(url, "attachment URL")?;
    Ok(Attachment {
        id,
        task_id,
        kind: AttachmentKind::Link { url },
        created_at: now,
    })
}

/// Attaches a file (already stored in the [`crate::ports::BlobStore`] under
/// `blob_key`) to a task. Rejects a blank (post-trim) filename or blob key.
pub fn attach_file(
    id: AttachmentId,
    task_id: TaskId,
    blob_key: impl AsRef<str>,
    filename: impl AsRef<str>,
    mime: impl Into<String>,
    size: u64,
    now: Timestamp,
) -> Result<Attachment, AppError> {
    let blob_key = non_blank(blob_key, "blob key")?;
    let filename = non_blank(filename, "filename")?;
    Ok(Attachment {
        id,
        task_id,
        kind: AttachmentKind::File {
            blob_key,
            filename,
            mime: mime.into(),
            size,
        },
        created_at: now,
    })
}

fn non_blank(raw: impl AsRef<str>, what: &str) -> Result<String, AppError> {
    let trimmed = raw.as_ref().trim();
    if trimmed.is_empty() {
        return Err(AppError::Invalid(format!("{what} must not be empty")));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tid() -> TaskId {
        TaskId::new(Uuid::from_u128(1))
    }

    fn ts(secs: i64) -> Timestamp {
        Timestamp::from_unix_seconds(secs).unwrap()
    }

    #[test]
    fn create_comment_builds_a_comment_with_no_edited_at() {
        let c = create_comment(
            CommentId::new(Uuid::from_u128(1)),
            tid(),
            UserId::new("alice"),
            "Looks good",
            ts(0),
        )
        .unwrap();
        assert_eq!(c.body, "Looks good");
        assert_eq!(c.edited_at, None);
    }

    #[test]
    fn create_comment_rejects_a_blank_body() {
        let result = create_comment(
            CommentId::new(Uuid::from_u128(1)),
            tid(),
            UserId::new("alice"),
            "   ",
            ts(0),
        );
        assert!(matches!(result, Err(AppError::Invalid(_))));
    }

    #[test]
    fn edit_comment_replaces_the_body_and_stamps_edited_at() {
        let c = create_comment(
            CommentId::new(Uuid::from_u128(1)),
            tid(),
            UserId::new("alice"),
            "first draft",
            ts(0),
        )
        .unwrap();
        let edited = edit_comment(&c, "final draft", ts(10)).unwrap();
        assert_eq!(edited.body, "final draft");
        assert_eq!(edited.edited_at, Some(ts(10)));
    }

    #[test]
    fn attach_link_rejects_a_blank_url() {
        let result = attach_link(AttachmentId::new(Uuid::from_u128(1)), tid(), "  ", ts(0));
        assert!(matches!(result, Err(AppError::Invalid(_))));
    }

    #[test]
    fn attach_file_builds_a_file_attachment() {
        let a = attach_file(
            AttachmentId::new(Uuid::from_u128(1)),
            tid(),
            "blobs/abc123",
            "photo.png",
            "image/png",
            2048,
            ts(0),
        )
        .unwrap();
        match a.kind {
            AttachmentKind::File {
                blob_key,
                filename,
                mime,
                size,
            } => {
                assert_eq!(blob_key, "blobs/abc123");
                assert_eq!(filename, "photo.png");
                assert_eq!(mime, "image/png");
                assert_eq!(size, 2048);
            }
            AttachmentKind::Link { .. } => panic!("expected a File attachment"),
        }
    }
}
