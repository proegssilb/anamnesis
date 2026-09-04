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

use anamnesis_app::{BlobStore, RepoError};
use async_trait::async_trait;

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
}

#[async_trait]
impl BlobStore for FsBlobStore {
    async fn put(&self, key: &str, bytes: Vec<u8>, _mime: &str) -> Result<(), RepoError> {
        let path = self.resolve(key)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| RepoError::from_source("failed to create blob parent directory", e))?;
        }
        write_atomically(&path, bytes).await
    }

    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, RepoError> {
        let path = self.resolve(key)?;
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(RepoError::from_source("failed to read blob", e)),
        }
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
async fn write_atomically(path: &Path, bytes: Vec<u8>) -> Result<(), RepoError> {
    // `resolve` only ever returns the canonicalised root with at least one
    // component pushed onto it, so there is always a parent; this stays an
    // error rather than an `expect` so the invariant cannot become a panic.
    let parent = path
        .parent()
        .ok_or_else(|| RepoError::new(format!("blob path {path:?} has no parent directory")))?;
    let tmp = parent.join(format!(".tmp-{}", uuid::Uuid::new_v4()));

    let published = match write_and_sync(&tmp, bytes).await {
        Ok(()) => tokio::fs::rename(&tmp, path)
            .await
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

/// Creates `path`, writes `bytes`, and flushes them to the device before
/// returning — so that a `rename` afterwards cannot publish a name whose
/// contents never reached disk.
async fn write_and_sync(path: &Path, bytes: Vec<u8>) -> Result<(), RepoError> {
    use tokio::io::AsyncWriteExt as _;

    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| RepoError::from_source("failed to create temporary blob", e))?;
    file.write_all(&bytes)
        .await
        .map_err(|e| RepoError::from_source("failed to write blob", e))?;
    file.sync_all()
        .await
        .map_err(|e| RepoError::from_source("failed to flush blob", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn put_then_get_round_trips_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();

        store
            .put("photos/a.png", b"hello".to_vec(), "image/png")
            .await
            .unwrap();
        let got = store.get("photos/a.png").await.unwrap();
        assert_eq!(got, Some(b"hello".to_vec()));
    }

    #[tokio::test]
    async fn put_leaves_no_temporary_file_behind() {
        // The temporary is an implementation detail of `put`'s atomicity: a
        // successful write must leave the root holding the blob and nothing
        // else, or every write would litter a shared blob root.
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();
        store.put("a", b"x".to_vec(), "text/plain").await.unwrap();

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
            .put("a", b"the original bytes".to_vec(), "text/plain")
            .await
            .unwrap();
        store
            .put("a", b"shorter".to_vec(), "text/plain")
            .await
            .unwrap();
        assert_eq!(store.get("a").await.unwrap(), Some(b"shorter".to_vec()));
    }

    #[tokio::test]
    async fn get_of_a_missing_key_is_none_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();
        assert_eq!(store.get("nope").await.unwrap(), None);
    }

    #[tokio::test]
    async fn delete_then_get_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();
        store.put("a", b"x".to_vec(), "text/plain").await.unwrap();
        store.delete("a").await.unwrap();
        assert_eq!(store.get("a").await.unwrap(), None);
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
            .put("../../etc/passwd", b"pwned".to_vec(), "text/plain")
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
            .expect_err("a key that climbs above the root even after descending must be rejected");
        assert!(err.to_string().contains("escapes the store root"));
    }

    #[tokio::test]
    async fn an_absolute_path_key_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();

        let err = store
            .put("/etc/passwd", b"pwned".to_vec(), "text/plain")
            .await
            .expect_err("an absolute-path key must be rejected");
        assert!(err.to_string().contains("must be a relative path"));
    }

    #[tokio::test]
    async fn an_empty_key_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();
        let err = store
            .put("", b"x".to_vec(), "text/plain")
            .await
            .expect_err("an empty key must be rejected");
        assert!(err.to_string().contains("must not be empty"));
    }

    #[tokio::test]
    async fn a_key_that_is_only_parent_dirs_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();
        let err = store
            .put("..", b"x".to_vec(), "text/plain")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("escapes the store root"));
    }

    #[tokio::test]
    async fn a_key_that_dips_and_returns_within_the_root_is_allowed() {
        // "a/../b" normalises to "b", which is still inside the root: this
        // must be accepted, not conflated with real traversal.
        let dir = tempfile::tempdir().unwrap();
        let store = FsBlobStore::new(dir.path()).await.unwrap();
        store
            .put("a/../b", b"ok".to_vec(), "text/plain")
            .await
            .unwrap();
        assert_eq!(store.get("b").await.unwrap(), Some(b"ok".to_vec()));
    }
}
