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
//! Both stream, in both directions: `BlobStore::put` takes a `ByteStream`
//! rather than a whole `Vec<u8>`, and `get`/`get_range` return one back. An
//! attachment's bytes are never fully resident in either adapter — `fs.rs`
//! bridges the stream straight to `tokio::io::copy`, and `s3.rs` uses
//! `object_store`'s real multipart upload (buffering only a small,
//! fixed-size peek-ahead prefix so most attachments still cost one API call
//! rather than three — see `S3BlobStore::put`'s own doc comment) and ranged
//! `GET`s. This is what makes a large `ANAMNESIS_MAX_BODY_BYTES` ceiling
//! (`docs/DEPLOYMENT.md` §5) safe to raise: peak memory per upload no
//! longer scales with it.

mod fs;
mod s3;

pub use fs::FsBlobStore;
pub use s3::{S3BlobStore, S3Settings};
