//! [`S3BlobStore`]: a [`BlobStore`] backed by an S3-compatible object store
//! — Garage (the deployment this was written for), MinIO, or S3 itself.
//!
//! This is what makes instances on *separate machines* possible
//! (`docs/DEPLOYMENT.md` §12): every other piece of shared state already
//! coordinates through the database, and attachment bytes were the last
//! thing pinning every instance to one filesystem.
//!
//! Keys are the same opaque strings [`super::FsBlobStore`] takes, and get
//! the same treatment in spirit: the store's own prefix is prepended and the
//! result must be a valid object key, so no caller-supplied key can name an
//! object outside the configured prefix. `object_store`'s [`Path`] is what
//! enforces that — it rejects `.` and `..` segments, empty segments, and
//! control characters outright rather than normalising them away.
//!
//! **Requests are path-style** (`{endpoint}/{bucket}/{key}`), which is what
//! self-hosted endpoints expect and what AWS still accepts. A virtual-hosted
//! endpoint (`{bucket}.s3.example.com`) is not configurable here; add it
//! when something actually needs it.

use anamnesis_app::{BlobInfo, BlobStore, ByteStream, ChunkedUpload, PartInfo, RepoError};
use async_trait::async_trait;
use futures_util::StreamExt as _;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::multipart::{MultipartStore, PartId};
use object_store::path::Path;
use object_store::{
    Attribute, AttributeValue, Attributes, GetOptions, GetRange, ObjectStore, ObjectStoreExt,
    PutMultipartOptions, PutOptions, WriteMultipart,
};

/// The connection details [`S3BlobStore`] needs beyond its
/// `s3://bucket/prefix` URL.
///
/// Deliberately **not** `#[derive(Debug)]`, for the same reason
/// `anamnesis_web::config::Config` is not: it holds a credential, and a
/// derived `Debug` would print it in full anywhere this reached a log line
/// (CWE-312). The impl below redacts it.
#[derive(Clone)]
pub struct S3Settings {
    /// The endpoint to talk to, e.g. `https://garage.example.com:3900`.
    /// `None` uses AWS's own regional endpoint, which is only right when the
    /// store really is S3 — Garage and MinIO always need this set.
    ///
    /// An `http://` endpoint switches the client to plaintext. That is a
    /// deliberate consequence of the scheme, not a separate knob: an
    /// operator who writes `http://` has said what they meant.
    pub endpoint: Option<String>,
    /// `None` leaves `object_store`'s own default (`us-east-1`). Garage
    /// accepts whatever region it was configured with and ignores the rest,
    /// but the value still has to match, because it is signed over.
    pub region: Option<String>,
    pub access_key_id: String,
    pub secret_access_key: String,
}

impl std::fmt::Debug for S3Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Settings")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .finish()
    }
}

/// A [`BlobStore`] holding attachment bytes as objects in one bucket.
#[derive(Debug)]
pub struct S3BlobStore {
    inner: AmazonS3,
    /// The key prefix every object sits under, already stripped of leading
    /// and trailing slashes. `None` means the bucket root.
    prefix: Option<String>,
}

impl S3BlobStore {
    /// Opens the store named by `url`, which is `s3://bucket` or
    /// `s3://bucket/some/prefix`.
    ///
    /// Nothing is contacted here: the S3 protocol has no connection to
    /// establish, so a wrong endpoint or a wrong credential surfaces on the
    /// first `put`/`get`, not at startup. Only the URL and the settings are
    /// validated.
    pub fn new(url: &str, settings: S3Settings) -> Result<Self, RepoError> {
        let (bucket, prefix) = split_url(url)?;
        Ok(Self {
            inner: build(bucket, settings)?,
            prefix,
        })
    }

    /// Turns a caller's blob key into the object key it names under this
    /// store's prefix, rejecting anything [`Path`] does not consider a
    /// well-formed key.
    fn location(&self, key: &str) -> Result<Path, RepoError> {
        if key.is_empty() {
            return Err(RepoError::new("blob key must not be empty"));
        }
        let full = match &self.prefix {
            Some(prefix) => format!("{prefix}/{key}"),
            None => key.to_string(),
        };
        Path::parse(&full)
            .map_err(|e| RepoError::from_source(format!("invalid blob key {key:?}"), e))
    }

    /// The reverse of [`Self::location`]: turns a listed object's full path
    /// back into the caller-facing key `get`/`delete` would accept.
    fn strip_object_prefix(&self, location: &Path) -> Result<String, RepoError> {
        let full = location.as_ref();
        match &self.prefix {
            Some(prefix) => full
                .strip_prefix(&format!("{prefix}/"))
                .map(str::to_string)
                .ok_or_else(|| {
                    RepoError::new(format!(
                        "listed object {full:?} outside the configured prefix {prefix:?}"
                    ))
                }),
            None => Ok(full.to_string()),
        }
    }
}

/// The peek-ahead buffer size for [`S3BlobStore::put`]: small enough to
/// keep peak memory bounded and independent of attachment size, large
/// enough to comfortably clear S3's 5 MiB minimum part size so an object
/// that turns out to need multipart upload never has to redo its first
/// part smaller than that minimum.
const PEEK_AHEAD_BYTES: usize = 8 * 1024 * 1024;

/// How many in-flight `UploadPart` requests [`S3BlobStore::put`] allows at
/// once, applying backpressure to the incoming stream past that point
/// rather than buffering unboundedly many parts in memory.
const MAX_CONCURRENT_PARTS: usize = 4;

#[async_trait]
impl BlobStore for S3BlobStore {
    async fn put(&self, key: &str, mut data: ByteStream<'_>, mime: &str) -> Result<u64, RepoError> {
        let location = self.location(key)?;
        // The MIME type is recorded on the object so that anything reading
        // the bucket directly (a backup tool, a browser hitting a presigned
        // URL) sees the same type the upload declared. Anamnesis itself
        // stores the type in the attachment row and does not read it back
        // from here.
        let mut attributes = Attributes::new();
        attributes.insert(
            Attribute::ContentType,
            AttributeValue::from(mime.to_string()),
        );

        // Buffer only the first `PEEK_AHEAD_BYTES`: if the object ends
        // within that, a single plain PUT is one API call instead of three
        // (create-multipart, one part, complete) -- worth it because most
        // attachments in real use are small. Only a stream that turns out
        // larger than the buffer pays for multipart upload.
        let (prefix, exhausted) = buffer_up_to(&mut data, PEEK_AHEAD_BYTES).await?;
        if exhausted {
            let options = PutOptions {
                attributes,
                ..Default::default()
            };
            let len = prefix.len() as u64;
            self.inner
                .put_opts(&location, prefix.into(), options)
                .await
                .map(|_| len)
                .map_err(|e| RepoError::from_source("failed to write blob", e))
        } else {
            let options = PutMultipartOptions {
                attributes,
                ..Default::default()
            };
            let upload = self
                .inner
                .put_multipart_opts(&location, options)
                .await
                .map_err(|e| RepoError::from_source("failed to start blob upload", e))?;
            let mut writer = WriteMultipart::new(upload);
            let already_read = prefix.len() as u64;
            writer.put(prefix.into());
            write_multipart(writer, data, already_read).await
        }
    }

    async fn get(&self, key: &str) -> Result<Option<ByteStream<'static>>, RepoError> {
        let location = self.location(key)?;
        let result = match self.inner.get(&location).await {
            Ok(result) => result,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(RepoError::from_source("failed to read blob", e)),
        };
        let stream = result
            .into_stream()
            .map(|r| r.map_err(std::io::Error::other));
        Ok(Some(Box::pin(stream)))
    }

    async fn get_range(
        &self,
        key: &str,
        start: u64,
        end: u64,
    ) -> Result<Option<ByteStream<'static>>, RepoError> {
        let location = self.location(key)?;
        let opts = GetOptions {
            range: Some(GetRange::Bounded(start..end + 1)),
            ..Default::default()
        };
        let result = match self.inner.get_opts(&location, opts).await {
            Ok(result) => result,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(RepoError::from_source("failed to read blob", e)),
        };
        let stream = result
            .into_stream()
            .map(|r| r.map_err(std::io::Error::other));
        Ok(Some(Box::pin(stream)))
    }

    async fn delete(&self, key: &str) -> Result<(), RepoError> {
        let location = self.location(key)?;
        match self.inner.delete(&location).await {
            // S3 itself answers a delete of a missing key with success, but
            // not every implementation does, and `FsBlobStore` treats it as
            // success too — so the port's behaviour cannot depend on which
            // backend is running.
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(RepoError::from_source("failed to delete blob", e)),
        }
    }

    async fn list(&self) -> Result<Vec<BlobInfo>, RepoError> {
        let prefix = self.prefix.as_ref().map(|p| Path::from(p.as_str()));
        let mut stream = self.inner.list(prefix.as_ref());
        let mut out = Vec::new();
        while let Some(meta) = stream.next().await {
            let meta = meta.map_err(|e| RepoError::from_source("failed to list blob store", e))?;
            out.push(BlobInfo {
                key: self.strip_object_prefix(&meta.location)?,
                last_modified: anamnesis_core::Timestamp::from_unix_seconds(
                    meta.last_modified.timestamp(),
                )
                .map_err(|e| {
                    RepoError::from_source("blob store returned an out-of-range modified time", e)
                })?,
            });
        }
        Ok(out)
    }
}

/// Built on `object_store`'s [`MultipartStore`] — the real S3 multipart
/// upload protocol, already stateless by design: a [`MultipartId`] plus
/// `path` addresses an in-progress upload from any client, any process,
/// indefinitely, exactly the property [`ChunkedUpload`] needs to let one
/// instance handle `begin` and a completely different one handle `put_part`
/// or `complete` (`docs/DEPLOYMENT.md` §12). Each part's `content_id`
/// carries the real per-part identifier `complete_multipart` requires back,
/// via [`PartInfo::content_id`] — recorded durably by the caller
/// (`anamnesis_app::AttachmentUploadRepository`), not remembered here.
///
/// [`MultipartId`]: object_store::MultipartId
#[async_trait]
impl ChunkedUpload for S3BlobStore {
    async fn begin(&self, key: &str, mime: &str) -> Result<String, RepoError> {
        let location = self.location(key)?;
        let mut attributes = Attributes::new();
        attributes.insert(
            Attribute::ContentType,
            AttributeValue::from(mime.to_string()),
        );
        let opts = PutMultipartOptions {
            attributes,
            ..Default::default()
        };
        self.inner
            .create_multipart_opts(&location, opts)
            .await
            .map_err(|e| RepoError::from_source("failed to start upload", e))
    }

    async fn put_part(
        &self,
        key: &str,
        token: &str,
        part_number: u32,
        mut data: ByteStream<'_>,
    ) -> Result<PartInfo, RepoError> {
        let location = self.location(key)?;
        // One part is one HTTP request's whole body, already bounded by
        // `ANAMNESIS_MAX_BODY_BYTES` well below what's safe to buffer --
        // `MultipartStore::put_part` needs the complete part as one
        // `PutPayload` regardless, unlike `BlobStore::put`'s own streaming.
        let mut buf = Vec::new();
        while let Some(chunk) = data.next().await {
            let chunk = chunk.map_err(|e| RepoError::from_source("failed to read part", e))?;
            buf.extend_from_slice(&chunk);
        }
        let size = buf.len() as u64;
        let part_idx = (part_number.saturating_sub(1)) as usize;
        let part_id = self
            .inner
            .put_part(&location, &token.to_string(), part_idx, buf.into())
            .await
            .map_err(|e| RepoError::from_source("failed to upload part", e))?;
        Ok(PartInfo {
            number: part_number,
            content_id: part_id.content_id,
            size,
        })
    }

    async fn complete(&self, key: &str, token: &str, parts: &[PartInfo]) -> Result<u64, RepoError> {
        let location = self.location(key)?;
        let part_ids = parts
            .iter()
            .map(|p| PartId {
                content_id: p.content_id.clone(),
            })
            .collect();
        self.inner
            .complete_multipart(&location, &token.to_string(), part_ids)
            .await
            .map_err(|e| RepoError::from_source("failed to complete upload", e))?;
        Ok(parts.iter().map(|p| p.size).sum())
    }

    async fn abort(&self, key: &str, token: &str) -> Result<(), RepoError> {
        let location = self.location(key)?;
        self.inner
            .abort_multipart(&location, &token.to_string())
            .await
            .map_err(|e| RepoError::from_source("failed to abort upload", e))
    }
}

/// Reads up to `cap` bytes off the front of `data`, without reading past
/// the first chunk that reaches or exceeds it. Returns the bytes read and
/// whether the stream was exhausted before reaching `cap` -- `true` means
/// the whole object fit in the buffer and [`S3BlobStore::put`] can do one
/// plain PUT; `false` means `data` has more to give and multipart upload is
/// needed, with `buf` becoming that upload's first part.
async fn buffer_up_to(data: &mut ByteStream<'_>, cap: usize) -> Result<(Vec<u8>, bool), RepoError> {
    let mut buf = Vec::new();
    while buf.len() < cap {
        match data.next().await {
            Some(Ok(chunk)) => buf.extend_from_slice(&chunk),
            Some(Err(e)) => {
                return Err(RepoError::from_source("failed to read upload stream", e));
            }
            None => return Ok((buf, true)),
        }
    }
    Ok((buf, false))
}

/// Streams the rest of `data` into an in-progress multipart upload,
/// continuing the running size count from `size` (the bytes already read
/// into the upload's first part by the caller). A failed read or a failed
/// part upload aborts the multipart upload before returning -- the
/// S3-shaped equivalent of `FsBlobStore::write_atomically`'s temp-file
/// cleanup: a partial upload must never become a visible object, nor linger
/// as a dangling incomplete one any longer than this can help.
async fn write_multipart(
    mut writer: WriteMultipart,
    mut data: ByteStream<'_>,
    mut size: u64,
) -> Result<u64, RepoError> {
    loop {
        match data.next().await {
            Some(Ok(chunk)) => {
                size += chunk.len() as u64;
                writer.put(chunk);
                if let Err(e) = writer.wait_for_capacity(MAX_CONCURRENT_PARTS).await {
                    let _ = writer.abort().await;
                    return Err(RepoError::from_source("failed to write blob", e));
                }
            }
            Some(Err(e)) => {
                let _ = writer.abort().await;
                return Err(RepoError::from_source("failed to read upload stream", e));
            }
            None => break,
        }
    }
    writer
        .finish()
        .await
        .map(|_| size)
        .map_err(|e| RepoError::from_source("failed to publish blob", e))
}

/// Splits `s3://bucket/prefix` into its bucket and its (optional) prefix.
fn split_url(url: &str) -> Result<(String, Option<String>), RepoError> {
    let rest = url.strip_prefix("s3://").ok_or_else(|| {
        RepoError::new(format!(
            "unsupported blob store URL {url:?}: expected an \"s3://bucket\" or \
             \"s3://bucket/prefix\" URL"
        ))
    })?;
    let (bucket, prefix) = match rest.split_once('/') {
        Some((bucket, prefix)) => (bucket, prefix.trim_matches('/')),
        None => (rest, ""),
    };
    if bucket.is_empty() {
        return Err(RepoError::new(format!(
            "blob store URL {url:?} names no bucket"
        )));
    }
    let prefix = (!prefix.is_empty()).then(|| prefix.to_string());
    Ok((bucket.to_string(), prefix))
}

/// Builds the client for `bucket` from `settings`.
fn build(bucket: String, settings: S3Settings) -> Result<AmazonS3, RepoError> {
    let mut builder = AmazonS3Builder::new()
        .with_bucket_name(bucket)
        // `BlobStore::delete` removes exactly one object, and left to itself
        // `object_store` would spend a `POST /?delete` bulk request to do it
        // -- an XML request body, an XML response, and an API some
        // S3-compatible servers do not implement at all. A plain
        // `DELETE /key` is core S3 that every provider supports, and for one
        // object it is strictly less machinery.
        .with_disable_bulk_delete(true)
        .with_access_key_id(settings.access_key_id)
        .with_secret_access_key(settings.secret_access_key);
    if let Some(region) = settings.region {
        builder = builder.with_region(region);
    }
    if let Some(endpoint) = settings.endpoint {
        let plaintext = endpoint.starts_with("http://");
        builder = builder.with_endpoint(endpoint).with_allow_http(plaintext);
    }
    builder
        .build()
        .map_err(|e| RepoError::from_source("failed to open the S3 blob store", e))
}

#[cfg(test)]
mod tests {
    //! Two kinds of test here, and neither reaches a real object store.
    //!
    //! URL and key handling is pure and is tested directly. The three port
    //! operations are tested against a `wiremock` server standing in for the
    //! endpoint: that proves the request this adapter *makes* (method, path,
    //! content type) and the answer it gives back for each response —
    //! including the 404 arm, which is the one piece of protocol behaviour
    //! the port depends on. It does not prove Anamnesis agrees with a real
    //! S3 implementation; `tests/blob_store_contract.rs` does that, against
    //! a live server, when one is configured.

    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ETAG: &str = "\"0123456789abcdef\"";
    /// RFC 2822, and the weekday has to be the real one for the date --
    /// `chrono` rejects a mismatch rather than ignoring it.
    const LAST_MODIFIED: &str = "Fri, 04 Sep 2026 12:00:00 GMT";

    /// A store pointed at `server`, holding objects under `blobs/att`.
    fn store_for(server: &MockServer) -> S3BlobStore {
        S3BlobStore::new(
            "s3://blobs/att",
            S3Settings {
                endpoint: Some(server.uri()),
                region: Some("garage".to_string()),
                access_key_id: "test-key".to_string(),
                secret_access_key: "test-secret".to_string(),
            },
        )
        .unwrap()
    }

    #[test]
    fn a_url_splits_into_bucket_and_prefix() {
        assert_eq!(
            split_url("s3://blobs/att/files").unwrap(),
            ("blobs".to_string(), Some("att/files".to_string()))
        );
        assert_eq!(
            split_url("s3://blobs").unwrap(),
            ("blobs".to_string(), None)
        );
        // A trailing slash names the bucket root, not an empty segment.
        assert_eq!(
            split_url("s3://blobs/").unwrap(),
            ("blobs".to_string(), None)
        );
    }

    #[test]
    fn a_url_that_is_not_an_s3_url_is_rejected() {
        let err = split_url("/var/lib/anamnesis/blobs").unwrap_err();
        assert!(err.to_string().contains("expected an \"s3://bucket\""));
        let err = split_url("s3:///att").unwrap_err();
        assert!(err.to_string().contains("names no bucket"));
    }

    #[test]
    fn keys_land_under_the_configured_prefix() {
        let store = S3BlobStore::new("s3://blobs/att", settings()).unwrap();
        assert_eq!(store.location("a.png").unwrap().as_ref(), "att/a.png");
        assert_eq!(
            store.location("deeper/a.png").unwrap().as_ref(),
            "att/deeper/a.png"
        );
    }

    #[test]
    fn a_key_that_climbs_out_of_the_prefix_is_rejected() {
        // The filesystem store rejects these as path traversal; here they
        // are simply not valid object keys. Same outcome, and it must stay
        // the same outcome, since the same caller keys reach both.
        let store = S3BlobStore::new("s3://blobs/att", settings()).unwrap();
        let err = store.location("../escaped").unwrap_err();
        assert!(err.to_string().contains("invalid blob key"));
        let err = store.location("").unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn settings_do_not_print_their_credential() {
        let printed = format!("{:?}", settings());
        assert!(printed.contains("test-key"), "{printed}");
        assert!(!printed.contains("test-secret"), "{printed}");
    }

    fn settings() -> S3Settings {
        S3Settings {
            endpoint: Some("http://localhost:3900".to_string()),
            region: None,
            access_key_id: "test-key".to_string(),
            secret_access_key: "test-secret".to_string(),
        }
    }

    /// Wraps a byte slice as the single-chunk [`ByteStream`] most of these
    /// tests need — well under `PEEK_AHEAD_BYTES`, so `put` always takes the
    /// plain-PUT path here unless a test says otherwise.
    fn once(bytes: &[u8]) -> ByteStream<'static> {
        let bytes = bytes::Bytes::copy_from_slice(bytes);
        Box::pin(futures_util::stream::once(async move { Ok(bytes) }))
    }

    async fn collect(mut stream: ByteStream<'static>) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(chunk) = stream.next().await {
            out.extend_from_slice(&chunk.unwrap());
        }
        out
    }

    #[tokio::test]
    async fn put_sends_the_bytes_and_the_declared_content_type() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/blobs/att/a.png"))
            .and(header("content-type", "image/png"))
            .respond_with(ResponseTemplate::new(200).append_header("ETag", ETAG))
            .expect(1)
            .mount(&server)
            .await;

        let written = store_for(&server)
            .put("a.png", once(b"hello"), "image/png")
            .await
            .unwrap();
        assert_eq!(written, 5);
    }

    #[tokio::test]
    async fn get_returns_the_object_bytes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/blobs/att/a.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("ETag", ETAG)
                    .append_header("Last-Modified", LAST_MODIFIED)
                    .set_body_bytes(b"hello".to_vec()),
            )
            .mount(&server)
            .await;

        let got = store_for(&server).get("a.png").await.unwrap().unwrap();
        assert_eq!(collect(got).await, b"hello");
    }

    #[tokio::test]
    async fn get_of_a_missing_object_is_none_not_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/blobs/att/gone.png"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        assert!(store_for(&server).get("gone.png").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn get_of_a_failing_endpoint_is_an_error_not_a_missing_blob() {
        // The distinction matters: `None` means "no such attachment" and
        // renders a 404 to the user, while a broken store must not be able
        // to make attachments look deleted.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/blobs/att/a.png"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let err = store_for(&server).get("a.png").await.err().unwrap();
        assert!(err.to_string().contains("failed to read blob"));
    }

    #[tokio::test]
    async fn get_range_sends_a_bounded_range_request() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/blobs/att/a.png"))
            .and(header("range", "bytes=2-4"))
            .respond_with(
                ResponseTemplate::new(206)
                    .append_header("ETag", ETAG)
                    .append_header("Last-Modified", LAST_MODIFIED)
                    .append_header("Content-Range", "bytes 2-4/5")
                    .set_body_bytes(b"llo".to_vec()),
            )
            .mount(&server)
            .await;

        let got = store_for(&server)
            .get_range("a.png", 2, 4)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(collect(got).await, b"llo");
    }

    #[tokio::test]
    async fn put_larger_than_the_peek_ahead_buffer_uses_multipart_upload() {
        // Anything past `PEEK_AHEAD_BYTES` must take the create/upload-part/
        // complete path rather than a single PUT -- this is the whole point
        // of streaming a large attachment instead of buffering it whole.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/blobs/att/big.bin"))
            .and(wiremock::matchers::query_param("uploads", ""))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><InitiateMultipartUploadResult><Bucket>blobs</Bucket><Key>att/big.bin</Key><UploadId>up-1</UploadId></InitiateMultipartUploadResult>",
                ),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/blobs/att/big.bin"))
            .respond_with(ResponseTemplate::new(200).append_header("ETag", ETAG))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/blobs/att/big.bin"))
            .and(wiremock::matchers::query_param("uploadId", "up-1"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><CompleteMultipartUploadResult><Bucket>blobs</Bucket><Key>att/big.bin</Key><ETag>\"whole\"</ETag></CompleteMultipartUploadResult>",
            ))
            .expect(1)
            .mount(&server)
            .await;

        let big = vec![0u8; PEEK_AHEAD_BYTES + 1024];
        let written = store_for(&server)
            .put("big.bin", once(&big), "application/octet-stream")
            .await
            .unwrap();
        assert_eq!(written, big.len() as u64);
    }

    #[tokio::test]
    async fn delete_removes_the_object() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/blobs/att/a.png"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        store_for(&server).delete("a.png").await.unwrap();
    }

    #[tokio::test]
    async fn list_returns_keys_stripped_of_the_configured_prefix() {
        let server = MockServer::start().await;
        let body = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
<Name>blobs</Name><Prefix>att</Prefix><KeyCount>1</KeyCount><MaxKeys>1000</MaxKeys>\
<IsTruncated>false</IsTruncated>\
<Contents><Key>att/a.png</Key><LastModified>2026-09-04T12:00:00.000Z</LastModified>\
<ETag>\"etag\"</ETag><Size>5</Size><StorageClass>STANDARD</StorageClass></Contents>\
</ListBucketResult>";
        Mock::given(method("GET"))
            .and(path("/blobs"))
            .and(wiremock::matchers::query_param("list-type", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;

        let listed = store_for(&server).list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].key, "a.png",
            "a listed object's key must be stripped of the store's own configured prefix"
        );
    }
}
