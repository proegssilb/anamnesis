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

use anamnesis_app::{BlobStore, RepoError};
use async_trait::async_trait;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path;
use object_store::{
    Attribute, AttributeValue, Attributes, ObjectStore, ObjectStoreExt, PutOptions,
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
}

#[async_trait]
impl BlobStore for S3BlobStore {
    async fn put(&self, key: &str, bytes: Vec<u8>, mime: &str) -> Result<(), RepoError> {
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
        let options = PutOptions {
            attributes,
            ..Default::default()
        };
        self.inner
            .put_opts(&location, bytes.into(), options)
            .await
            .map(|_| ())
            .map_err(|e| RepoError::from_source("failed to write blob", e))
    }

    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, RepoError> {
        let location = self.location(key)?;
        let result = match self.inner.get(&location).await {
            Ok(result) => result,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(RepoError::from_source("failed to read blob", e)),
        };
        // The whole object, resident: that is what the port asks for. See
        // the module doc comment on `super` for the ceiling this implies.
        let bytes = result
            .bytes()
            .await
            .map_err(|e| RepoError::from_source("failed to read blob", e))?;
        Ok(Some(bytes.to_vec()))
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

        store_for(&server)
            .put("a.png", b"hello".to_vec(), "image/png")
            .await
            .unwrap();
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

        let got = store_for(&server).get("a.png").await.unwrap();
        assert_eq!(got, Some(b"hello".to_vec()));
    }

    #[tokio::test]
    async fn get_of_a_missing_object_is_none_not_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/blobs/att/gone.png"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        assert_eq!(store_for(&server).get("gone.png").await.unwrap(), None);
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

        let err = store_for(&server).get("a.png").await.unwrap_err();
        assert!(err.to_string().contains("failed to read blob"));
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
}
