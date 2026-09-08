//! [`FsBlobStore`]: a [`BlobStore`] backed by the local filesystem
//! (`docs/DOMAIN.md` §3: "local filesystem first, S3-shaped later").
//!
//! Keys are opaque strings minted by the caller and recorded as an
//! [`anamnesis_app::AttachmentKind::File`]'s `blob_key` — this adapter's own
//! contribution is turning that string into a path *underneath its
//! configured root and nowhere else*, no matter what the caller's key
//! contains. A key is rejected outright (not silently sanitised) if, once
//! joined onto the root and lexically normalised, it would resolve outside
//! it — a `../../etc/passwd`-shaped key, an absolute-path key that would
//! otherwise replace the join, or a key containing a NUL byte all fall out
//! of the same one check (see [`resolve`]).

use std::path::{Component, Path, PathBuf};

use anamnesis_app::{BlobStore, ByteStream, ChunkedUpload, PartInfo, RepoError};
use async_trait::async_trait;
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};

/// A [`BlobStore`] rooted at one directory on the local filesystem.
#[derive(Debug, Clone)]
pub struct FsBlobStore {
    root: PathBuf,
}

impl FsBlobStore {
    /// Roots a new store at `root`, creating the directory (and any missing
    /// parents) if it does not already exist.
    pub async fn new(root: impl Into<PathBuf>) -> Result<Self, RepoError> {
        let root = root.into();
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|e| RepoError::from_source("failed to create blob store root", e))?;
        let root = tokio::fs::canonicalize(&root)
            .await
            .map_err(|e| RepoError::from_source("failed to canonicalize blob store root", e))?;
        Ok(Self { root })
    }

    /// Resolves `key` to a path strictly underneath [`Self::root`], or
    /// rejects it.
    ///
    /// A key is walked component-by-component: a `..` that would climb
    /// above the root, a root/prefix component (an absolute path, or — on
    /// Windows — a drive letter) that would escape the join entirely, or an
    /// empty key are all rejected. This is a *lexical* check (it does not
    /// require the target to already exist, unlike canonicalizing the full
    /// path), which is exactly what `put` needs for a file that does not
    /// exist yet.
    fn resolve(&self, key: &str) -> Result<PathBuf, RepoError> {
        if key.is_empty() {
            return Err(RepoError::new("blob key must not be empty"));
        }
        let mut resolved = self.root.clone();
        let mut depth: u32 = 0;
        for component in Path::new(key).components() {
            match component {
                Component::Normal(part) => {
                    resolved.push(part);
                    depth += 1;
                }
                Component::CurDir => {}
                Component::ParentDir => {
                    if depth == 0 {
                        return Err(RepoError::new(format!(
                            "blob key {key:?} escapes the store root"
                        )));
                    }
                    depth -= 1;
                    resolved.pop();
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(RepoError::new(format!(
                        "blob key {key:?} must be a relative path"
                    )));
                }
            }
        }
        if depth == 0 {
            return Err(RepoError::new(format!(
                "blob key {key:?} does not name a file"
            )));
        }
        Ok(resolved)
    }

    /// The staging directory for a chunked upload's parts, named by its
    /// opaque token. Tokens are always minted by [`ChunkedUpload::begin`]
    /// (a fresh UUID), never user-supplied, but this still rejects a path
    /// separator outright rather than trusting that — the same
    /// defense-in-depth spirit as [`Self::resolve`].
    fn staging_dir(&self, token: &str) -> Result<PathBuf, RepoError> {
        if token.is_empty() || token.contains('/') || token.contains('\\') {
            return Err(RepoError::new(format!("invalid upload token {token:?}")));
        }
        Ok(self.root.join(".uploads").join(token))
    }
}

#[async_trait]
impl BlobStore for FsBlobStore {
    async fn put(&self, key: &str, data: ByteStream<'_>, _mime: &str) -> Result<u64, RepoError> {
        let path = self.resolve(key)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| RepoError::from_source("failed to create blob parent directory", e))?;
        }
        write_atomically(&path, data).await
    }

    async fn get(&self, key: &str) -> Result<Option<ByteStream<'static>>, RepoError> {
        let path = self.resolve(key)?;
        match tokio::fs::File::open(&path).await {
            Ok(file) => Ok(Some(Box::pin(tokio_util::io::ReaderStream::new(file)))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(RepoError::from_source("failed to read blob", e)),
        }
    }

    async fn get_range(
        &self,
        key: &str,
        start: u64,
        end: u64,
    ) -> Result<Option<ByteStream<'static>>, RepoError> {
        let path = self.resolve(key)?;
        let mut file = match tokio::fs::File::open(&path).await {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(RepoError::from_source("failed to read blob", e)),
        };
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|e| RepoError::from_source("failed to seek blob", e))?;
        let limited = file.take(end - start + 1);
        Ok(Some(Box::pin(tokio_util::io::ReaderStream::new(limited))))
    }

    async fn delete(&self, key: &str) -> Result<(), RepoError> {
        let path = self.resolve(key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(RepoError::from_source("failed to delete blob", e)),
        }
    }
}

/// A chunked upload's parts are staged as `part-<number>` files under a
/// per-upload directory, and [`ChunkedUpload::complete`] concatenates them
/// (in the order the caller's `parts` names, not filesystem order) into the
/// final blob via the same publish-by-rename discipline [`BlobStore::put`]
/// uses — a reader can never observe a partially assembled blob.
/// `content_id` is unused (always empty): `FsBlobStore` orders parts purely
/// by the caller-supplied `parts` list, needing no backend-side manifest.
#[async_trait]
impl ChunkedUpload for FsBlobStore {
    async fn begin(&self, _key: &str, _mime: &str) -> Result<String, RepoError> {
        let token = uuid::Uuid::new_v4().to_string();
        let dir = self.staging_dir(&token)?;
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| RepoError::from_source("failed to start upload", e))?;
        Ok(token)
    }

    async fn put_part(
        &self,
        _key: &str,
        token: &str,
        part_number: u32,
        data: ByteStream<'_>,
    ) -> Result<PartInfo, RepoError> {
        let dir = self.staging_dir(token)?;
        let path = dir.join(format!("part-{part_number:08}"));
        // Atomically published, exactly like a single-request blob: a part
        // upload interrupted mid-transfer must not leave a truncated
        // `part-*` file for `complete` to silently assemble into the blob.
        let size = write_atomically(&path, data).await?;
        Ok(PartInfo {
            number: part_number,
            content_id: String::new(),
            size,
        })
    }

    async fn complete(&self, key: &str, token: &str, parts: &[PartInfo]) -> Result<u64, RepoError> {
        let dest = self.resolve(key)?;
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| RepoError::from_source("failed to create blob parent directory", e))?;
        }
        let dir = self.staging_dir(token)?;
        let size = concatenate_parts(&dir, &dest, parts).await?;
        let _ = tokio::fs::remove_dir_all(&dir).await;
        Ok(size)
    }

    async fn abort(&self, _key: &str, token: &str) -> Result<(), RepoError> {
        let dir = self.staging_dir(token)?;
        let _ = tokio::fs::remove_dir_all(&dir).await;
        Ok(())
    }
}

/// Streams each `part-<number>` file named by `parts`, in that order, into
/// a temporary file next to `dest`, then publishes it the same
/// atomically-by-rename way [`write_atomically`] does for a single-request
/// upload.
async fn concatenate_parts(
    staging_dir: &Path,
    dest: &Path,
    parts: &[PartInfo],
) -> Result<u64, RepoError> {
    let parent = dest
        .parent()
        .ok_or_else(|| RepoError::new(format!("blob path {dest:?} has no parent directory")))?;
    let tmp = parent.join(format!(".tmp-{}", uuid::Uuid::new_v4()));

    let result = concatenate_into(staging_dir, &tmp, parts).await;
    let published = match result {
        Ok(size) => tokio::fs::rename(&tmp, dest)
            .await
            .map(|()| size)
            .map_err(|e| RepoError::from_source("failed to publish blob", e)),
        Err(e) => Err(e),
    };
    if published.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    published
}

async fn concatenate_into(
    staging_dir: &Path,
    tmp: &Path,
    parts: &[PartInfo],
) -> Result<u64, RepoError> {
    let mut out = tokio::fs::File::create(tmp)
        .await
        .map_err(|e| RepoError::from_source("failed to create temporary blob", e))?;
    let mut total = 0u64;
    for part in parts {
        let part_path = staging_dir.join(format!("part-{:08}", part.number));
        let mut part_file = tokio::fs::File::open(&part_path)
            .await
            .map_err(|e| RepoError::from_source("failed to read upload part", e))?;
        total += tokio::io::copy(&mut part_file, &mut out)
            .await
            .map_err(|e| RepoError::from_source("failed to assemble blob", e))?;
    }
    out.sync_all()
        .await
        .map_err(|e| RepoError::from_source("failed to flush blob", e))?;
    Ok(total)
}

/// Writes `bytes` to `path` by publishing it under its final name only once
/// it is complete: the bytes go to a uniquely named temporary file in the
/// *same directory* (so the `rename` is within one filesystem, and therefore
/// atomic), are flushed to disk, and only then take the real name.
///
/// A plain write is not good enough the moment more than one process shares
/// a blob root — which is exactly what a same-machine multi-instance
/// deployment does (`docs/DEPLOYMENT.md` §12). A writer interrupted partway
/// through, on a slow or remote filesystem, leaves a file that exists and is
/// readable but is short, and `get` has no way to tell that from a whole
/// blob: it would serve the truncated bytes as though they were the
/// attachment. With this, a reader sees either no file or the complete one.
///
/// A crash between the two steps leaves a `.tmp-*` file behind. That is the
/// deliberate trade — a stray temporary is inert and collectable, a
/// truncated blob is served — and it is what the blob GC sweep this plan
/// leaves a slot for would remove.
async fn write_atomically(path: &Path, data: ByteStream<'_>) -> Result<u64, RepoError> {
    // `resolve` only ever returns the canonicalised root with at least one
    // component pushed onto it, so there is always a parent; this stays an
    // error rather than an `expect` so the invariant cannot become a panic.
    let parent = path
        .parent()
        .ok_or_else(|| RepoError::new(format!("blob path {path:?} has no parent directory")))?;
    let tmp = parent.join(format!(".tmp-{}", uuid::Uuid::new_v4()));

    let published = match write_and_sync(&tmp, data).await {
        Ok(written) => tokio::fs::rename(&tmp, path)
            .await
            .map(|()| written)
            .map_err(|e| RepoError::from_source("failed to publish blob", e)),
        Err(e) => Err(e),
    };
    if published.is_err() {
        // Best effort: the error being returned is the interesting one, and
        // a leftover temporary is harmless either way.
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    published
}

/// Creates `path`, streams `data` into it, and flushes the result to the
/// device before returning — so that a `rename` afterwards cannot publish a
/// name whose contents never reached disk. Returns the number of bytes
/// written, since the caller (the port's `put`) never knows the total size
/// upfront.
async fn write_and_sync(path: &Path, data: ByteStream<'_>) -> Result<u64, RepoError> {
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| RepoError::from_source("failed to create temporary blob", e))?;
    let mut reader = tokio_util::io::StreamReader::new(data);
    let written = tokio::io::copy(&mut reader, &mut file)
        .await
        .map_err(|e| RepoError::from_source("failed to write blob", e))?;
    file.sync_all()
        .await
        .map_err(|e| RepoError::from_source("failed to flush blob", e))?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt as _;

    /// Wraps a byte slice as the single-chunk [`ByteStream`] a test `put`
    /// needs -- real multi-chunk behaviour is exercised in
    /// `anamnesis-adapters/tests/blob_store_contract.rs`, shared by both
    /// backends.
    fn once(bytes: &[u8]) -> ByteStream<'static> {
        let bytes = bytes::Bytes::copy_from_slice(bytes);
        Box::pin(futures_util::stream::once(async move { Ok(bytes) }))
    }

    /// Drains a returned [`ByteStream`] into an owned `Vec<u8>` for
    /// assertions.
    async fn collect(mut stream: ByteStream<'static>) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(chunk) = stream.next().await {
            out.extend_from_slice(&chunk.unwrap());
        }
        out
    }

    #[tokio::test]
    async fn put_then_get_round_trips_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();

        store
            .put("photos/a.png", once(b"hello"), "image/png")
            .await
            .unwrap();
        let got = store.get("photos/a.png").await.unwrap().unwrap();
        assert_eq!(collect(got).await, b"hello");
    }

    #[tokio::test]
    async fn put_leaves_no_temporary_file_behind() {
        // The temporary is an implementation detail of `put`'s atomicity: a
        // successful write must leave the root holding the blob and nothing
        // else, or every write would litter a shared blob root.
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();
        store.put("a", once(b"x"), "text/plain").await.unwrap();

        let mut names = Vec::new();
        let mut entries = tokio::fs::read_dir(dir.path()).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        assert_eq!(names, vec!["a".to_string()]);
    }

    #[tokio::test]
    async fn put_over_an_existing_key_replaces_it_whole() {
        // Publication is a rename, which must overwrite rather than fail --
        // and must not leave the two writes interleaved.
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();
        store
            .put("a", once(b"the original bytes"), "text/plain")
            .await
            .unwrap();
        store
            .put("a", once(b"shorter"), "text/plain")
            .await
            .unwrap();
        let got = store.get("a").await.unwrap().unwrap();
        assert_eq!(collect(got).await, b"shorter");
    }

    #[tokio::test]
    async fn get_of_a_missing_key_is_none_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();
        assert!(store.get("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_then_get_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();
        store.put("a", once(b"x"), "text/plain").await.unwrap();
        store.delete("a").await.unwrap();
        assert!(store.get("a").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_of_a_missing_key_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();
        store.delete("never-existed").await.unwrap();
    }

    #[tokio::test]
    async fn path_traversal_via_parent_dir_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();

        let err = store
            .put("../../etc/passwd", once(b"pwned"), "text/plain")
            .await
            .expect_err("a key climbing above the root must be rejected");
        assert!(err.to_string().contains("escapes the store root"));

        // Nothing was written outside the root.
        assert!(!dir.path().parent().unwrap().join("etc/passwd").exists());
    }

    #[tokio::test]
    async fn path_traversal_disguised_inside_a_deeper_key_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();

        // Climbs out from under `a/` and back above the root entirely.
        let err = store
            .get("a/../../escaped")
            .await
            .err()
            .expect("a key that climbs above the root even after descending must be rejected");
        assert!(err.to_string().contains("escapes the store root"));
    }

    #[tokio::test]
    async fn an_absolute_path_key_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();

        let err = store
            .put("/etc/passwd", once(b"pwned"), "text/plain")
            .await
            .expect_err("an absolute-path key must be rejected");
        assert!(err.to_string().contains("must be a relative path"));
    }

    #[tokio::test]
    async fn an_empty_key_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();
        let err = store
            .put("", once(b"x"), "text/plain")
            .await
            .expect_err("an empty key must be rejected");
        assert!(err.to_string().contains("must not be empty"));
    }

    #[tokio::test]
    async fn a_key_that_is_only_parent_dirs_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();
        let err = store.put("..", once(b"x"), "text/plain").await.unwrap_err();
        assert!(err.to_string().contains("escapes the store root"));
    }

    #[tokio::test]
    async fn a_key_that_dips_and_returns_within_the_root_is_allowed() {
        // "a/../b" normalises to "b", which is still inside the root: this
        // must be accepted, not conflated with real traversal.
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();
        store
            .put("a/../b", once(b"ok"), "text/plain")
            .await
            .unwrap();
        let got = store.get("b").await.unwrap().unwrap();
        assert_eq!(collect(got).await, b"ok");
    }

    #[tokio::test]
    async fn get_range_returns_the_requested_slice() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();
        store
            .put("a", once(b"0123456789"), "text/plain")
            .await
            .unwrap();

        let got = store.get_range("a", 2, 5).await.unwrap().unwrap();
        assert_eq!(collect(got).await, b"2345");
    }

    #[tokio::test]
    async fn get_range_of_a_missing_key_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();
        assert!(store.get_range("nope", 0, 3).await.unwrap().is_none());
    }
}
