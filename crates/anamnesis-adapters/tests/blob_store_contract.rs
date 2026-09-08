//! The `BlobStore` contract, exercised once and run against both backends
//! so they cannot drift — the same shape as `sql_store_contract.rs`, and for
//! the same reason: a deployment picks its backend from a URL scheme
//! (`docs/DEPLOYMENT.md` §2), so a difference between the two is a
//! difference a user would meet by moving machines.
//!
//! `FsBlobStore` runs against a temporary directory. `S3BlobStore` runs
//! against a live S3-compatible server when `ANAMNESIS_TEST_S3_URL` is set
//! (a Garage or MinIO in a container is enough — `just test-adapters-s3`);
//! it is `#[ignore]`d otherwise so `cargo test` stays green with no object
//! store running. The in-crate `wiremock` tests cover what the adapter
//! *sends* without a server; only this one proves a real implementation
//! answers the way the port expects.

use bytes::Bytes;
use futures_util::StreamExt;

use anamnesis_adapters::{FsBlobStore, S3BlobStore, S3Settings};
use anamnesis_app::{BlobStore, ByteStream};

/// Wraps a byte slice as a single-chunk [`ByteStream`].
fn once(bytes: &[u8]) -> ByteStream<'static> {
    let bytes = Bytes::copy_from_slice(bytes);
    Box::pin(futures_util::stream::once(async move { Ok(bytes) }))
}

/// Drains a returned [`ByteStream`] into an owned `Vec<u8>` for assertions.
async fn collect(stream: Option<ByteStream<'static>>) -> Option<Vec<u8>> {
    let mut stream = stream?;
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        out.extend_from_slice(&chunk.unwrap());
    }
    Some(out)
}

/// Every promise [`BlobStore`] makes, in the order a caller meets them.
///
/// `keyspace` is a prefix unique to this run: the filesystem store gets a
/// fresh directory each time, but a real bucket usually does not, and two
/// runs sharing an object key would make this contract's assertions depend
/// on which ran last.
async fn contract(store: &dyn BlobStore, keyspace: &str) {
    let key = format!("{keyspace}/report.pdf");
    let nested = format!("{keyspace}/deeper/still/report.pdf");

    // A key nobody has written is absent, not an error: this is what makes
    // a missing attachment a 404 rather than a 500.
    assert_eq!(collect(store.get(&key).await.unwrap()).await, None);

    let written = store
        .put(&key, once(b"the original bytes"), "application/pdf")
        .await
        .unwrap();
    assert_eq!(written, "the original bytes".len() as u64);
    assert_eq!(
        collect(store.get(&key).await.unwrap()).await,
        Some(b"the original bytes".to_vec())
    );

    // Overwriting replaces the object whole. The shorter second write is
    // deliberate: a backend that wrote in place rather than atomically would
    // leave the tail of the first write behind, and this would catch it.
    store
        .put(&key, once(b"shorter"), "application/pdf")
        .await
        .unwrap();
    assert_eq!(
        collect(store.get(&key).await.unwrap()).await,
        Some(b"shorter".to_vec())
    );

    // Keys with several segments are ordinary keys, not a directory feature
    // one backend has and the other does not.
    store
        .put(&nested, once(b"nested"), "application/pdf")
        .await
        .unwrap();
    assert_eq!(
        collect(store.get(&nested).await.unwrap()).await,
        Some(b"nested".to_vec())
    );

    store.delete(&key).await.unwrap();
    assert_eq!(collect(store.get(&key).await.unwrap()).await, None);

    // Deleting what is already gone is success on both backends — an
    // attachment row removed twice must not fail the second time.
    store.delete(&key).await.unwrap();

    // And the keys neither backend will accept.
    assert!(store.get("").await.is_err());
    assert!(store.get("../escaped").await.is_err());

    store.delete(&nested).await.unwrap();

    multi_chunk_round_trip(store, keyspace).await;
    mid_stream_failure_leaves_no_partial_blob(store, keyspace).await;
    get_range_round_trip(store, keyspace).await;
}

/// A larger, multi-chunk upload — several distinct `Bytes` chunks totalling
/// well past any single-buffer shortcut either backend might take — proves
/// `put` really does stream rather than silently reassembling one big
/// buffer first, and that its returned size is the true total.
async fn multi_chunk_round_trip(store: &dyn BlobStore, keyspace: &str) {
    let key = format!("{keyspace}/multi-chunk.bin");
    const CHUNK: usize = 4 * 1024 * 1024;
    let chunks: Vec<Bytes> = (0..3).map(|n| Bytes::from(vec![n as u8; CHUNK])).collect();
    let total: Vec<u8> = chunks.iter().flat_map(|c| c.to_vec()).collect();
    let stream: ByteStream<'static> =
        Box::pin(futures_util::stream::iter(chunks.into_iter().map(Ok)));

    let written = store
        .put(&key, stream, "application/octet-stream")
        .await
        .unwrap();
    assert_eq!(written, total.len() as u64);
    assert_eq!(collect(store.get(&key).await.unwrap()).await, Some(total));

    store.delete(&key).await.unwrap();
}

/// A stream that fails partway through must leave `put` erroring, and must
/// leave the store exactly as it was before the attempt — proving the
/// abort/atomic-publish guarantee holds for a streaming write, not only a
/// whole-buffer one.
async fn mid_stream_failure_leaves_no_partial_blob(store: &dyn BlobStore, keyspace: &str) {
    let key = format!("{keyspace}/mid-stream-failure.bin");
    let failing: ByteStream<'static> = Box::pin(futures_util::stream::iter(vec![
        Ok(Bytes::from_static(b"partial")),
        Err(std::io::Error::other("boom")),
    ]));
    let before = collect(store.get(&key).await.unwrap()).await;

    let err = store.put(&key, failing, "application/octet-stream").await;
    assert!(err.is_err(), "a mid-stream read failure must fail `put`");

    let after = collect(store.get(&key).await.unwrap()).await;
    assert_eq!(
        before, after,
        "a failed put must not change what a following get sees"
    );
}

/// A ranged read against a known multi-chunk blob returns exactly the
/// requested byte span.
async fn get_range_round_trip(store: &dyn BlobStore, keyspace: &str) {
    let key = format!("{keyspace}/range.bin");
    let bytes: Vec<u8> = (0u8..=255).collect();
    store
        .put(&key, once(&bytes), "application/octet-stream")
        .await
        .unwrap();

    let middle = store.get_range(&key, 10, 19).await.unwrap();
    assert_eq!(collect(middle).await, Some(bytes[10..=19].to_vec()));

    let end = store.get_range(&key, 250, 255).await.unwrap();
    assert_eq!(collect(end).await, Some(bytes[250..=255].to_vec()));

    assert!(store.get_range("nope", 0, 3).await.unwrap().is_none());

    store.delete(&key).await.unwrap();
}

#[tokio::test]
async fn fs_blob_store_contract() {
    let dir = tempfile::tempdir().expect("create temp blob dir");
    let store = FsBlobStore::new(dir.path())
        .await
        .expect("create temp blob store");

    contract(&store, "run").await;
}

#[tokio::test]
#[ignore = "requires a live S3-compatible server; set ANAMNESIS_TEST_S3_URL and pass --ignored"]
async fn s3_blob_store_contract() {
    let Ok(url) = std::env::var("ANAMNESIS_TEST_S3_URL") else {
        eprintln!("skipping s3_blob_store_contract: ANAMNESIS_TEST_S3_URL is not set");
        return;
    };

    let settings = S3Settings {
        endpoint: Some(env_or(
            "ANAMNESIS_TEST_S3_ENDPOINT",
            "http://localhost:9000",
        )),
        region: Some(env_or("ANAMNESIS_TEST_S3_REGION", "us-east-1")),
        access_key_id: env_or("ANAMNESIS_TEST_S3_ACCESS_KEY_ID", "minioadmin"),
        secret_access_key: env_or("ANAMNESIS_TEST_S3_SECRET_ACCESS_KEY", "minioadmin"),
    };
    let store = S3BlobStore::new(&url, settings).expect("open the test S3 blob store");

    contract(&store, &format!("run-{}", uuid::Uuid::new_v4())).await;
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}
