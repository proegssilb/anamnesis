//! The two [`anamnesis_app::BlobStore`] backends for attachment bytes
//! (`docs/DOMAIN.md` §3: "local filesystem first, S3-shaped later").
//!
//! - [`FsBlobStore`] — a directory on the local filesystem. The default, and
//!   all a single instance or several instances *on one machine* need, since
//!   they share the directory.
//! - [`S3BlobStore`] — an S3-compatible object store (Garage, MinIO, S3
//!   itself). What instances on *separate* machines need, because a local
//!   directory is exactly what they do not share (`docs/DEPLOYMENT.md` §12).
//!
//! Which one runs is decided by the scheme of `ANAMNESIS_BLOB_ROOT` in
//! `anamnesis-web`'s `open_blob_store`, mirroring how [`crate::SqlStore`]
//! dispatches on the database URL: an `s3://` URL selects the object store,
//! anything else is a filesystem path.
//!
//! Both are whole-object stores because the port is — `put` takes a `Vec<u8>`
//! and `get` returns one — so the S3 backend buffers each object in memory
//! rather than streaming it, and cannot use multipart uploads or range GETs.
//! That is a property of the port, not of this adapter: moving to a streaming
//! shape is a decision about the attachment size to support, and is out of
//! scope here (`docs/DEPLOYMENT.md` §5 sizes the resulting ceiling).

mod fs;
mod s3;

pub use fs::FsBlobStore;
pub use s3::{S3BlobStore, S3Settings};
