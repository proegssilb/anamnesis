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

use anamnesis_adapters::{FsBlobStore, S3BlobStore, S3Settings};
use anamnesis_app::BlobStore;

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
    assert_eq!(store.get(&key).await.unwrap(), None);

    store
        .put(&key, b"the original bytes".to_vec(), "application/pdf")
        .await
        .unwrap();
    assert_eq!(
        store.get(&key).await.unwrap(),
        Some(b"the original bytes".to_vec())
    );

    // Overwriting replaces the object whole. The shorter second write is
    // deliberate: a backend that wrote in place rather than atomically would
    // leave the tail of the first write behind, and this would catch it.
    store
        .put(&key, b"shorter".to_vec(), "application/pdf")
        .await
        .unwrap();
    assert_eq!(store.get(&key).await.unwrap(), Some(b"shorter".to_vec()));

    // Keys with several segments are ordinary keys, not a directory feature
    // one backend has and the other does not.
    store
        .put(&nested, b"nested".to_vec(), "application/pdf")
        .await
        .unwrap();
    assert_eq!(store.get(&nested).await.unwrap(), Some(b"nested".to_vec()));

    store.delete(&key).await.unwrap();
    assert_eq!(store.get(&key).await.unwrap(), None);

    // Deleting what is already gone is success on both backends — an
    // attachment row removed twice must not fail the second time.
    store.delete(&key).await.unwrap();

    // And the keys neither backend will accept.
    assert!(store.get("").await.is_err());
    assert!(store.get("../escaped").await.is_err());

    store.delete(&nested).await.unwrap();
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
