use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use base64::Engine as _;
use distribution::api::{AppState, router};
use distribution::migration;
use distribution::registry::Registry;
use distribution::storage::filesystem::FilesystemStorage;
use sea_orm::ConnectionTrait;
use sea_orm::{ConnectOptions, Database};
use sea_orm_migration::MigratorTrait;
use sha2::Digest as _;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

async fn setup() -> (axum::Router, TempDir) {
    let tmp_dir = TempDir::new().unwrap();
    let storage_dir = tmp_dir.path().join("storage");
    let db_path = tmp_dir.path().join("test.db");

    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let db = Database::connect(ConnectOptions::new(&db_url))
        .await
        .unwrap();

    db.execute_unprepared("PRAGMA foreign_keys = ON")
        .await
        .unwrap();
    db.execute_unprepared("PRAGMA journal_mode = WAL")
        .await
        .unwrap();

    migration::Migrator::up(&db, None).await.unwrap();

    let storage = Arc::new(FilesystemStorage::new(storage_dir).await.unwrap());
    let registry = Arc::new(Registry::new(db, storage));
    let state = AppState {
        registry,
        auth_config: distribution::config::AuthConfig::default(),
    };
    let app = router(state);

    (app, tmp_dir)
}

fn sha256_digest(data: &[u8]) -> String {
    let hash = sha2::Sha256::digest(data);
    format!("sha256:{}", hex::encode(hash))
}

#[tokio::test]
async fn test_v2_check() {
    let (app, _tmp) = setup().await;
    let req = Request::builder().uri("/v2/").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("Docker-Distribution-API-Version")
            .unwrap(),
        "registry/2.0"
    );
}

#[tokio::test]
async fn test_healthz() {
    let (app, _tmp) = setup().await;
    let req = Request::builder()
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_readyz() {
    let (app, _tmp) = setup().await;
    let req = Request::builder()
        .uri("/readyz")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_blob_not_found() {
    let (app, _tmp) = setup().await;
    let digest = format!("sha256:{}", "a".repeat(64));
    let req = Request::builder()
        .uri(format!("/v2/test/blobs/{digest}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_manifest_not_found() {
    let (app, _tmp) = setup().await;
    let req = Request::builder()
        .uri("/v2/test/manifests/latest")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_tag_list_empty_repo() {
    let (app, _tmp) = setup().await;
    let req = Request::builder()
        .uri("/v2/test/tags/list")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_catalog_empty() {
    let (app, _tmp) = setup().await;
    let req = Request::builder()
        .uri("/v2/_catalog")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["repositories"], serde_json::json!([]));
}

/// Full push-pull cycle: upload blob -> push manifest -> pull manifest -> pull blob
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn test_push_pull_cycle() {
    let (app, _tmp) = setup().await;

    let blob_content = b"hello world blob content";
    let blob_digest = sha256_digest(blob_content);

    // 1. Monolithic blob upload
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/v2/test/repo/blobs/uploads/?digest={blob_digest}"))
        .body(Body::from(blob_content.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "monolithic upload should return 201"
    );

    // 2. HEAD blob to verify it exists
    let req = Request::builder()
        .method(Method::HEAD)
        .uri(format!("/v2/test/repo/blobs/{blob_digest}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("Docker-Content-Digest")
            .unwrap()
            .to_str()
            .unwrap(),
        blob_digest
    );

    // 3. Upload config blob
    let config_content = b"{}";
    let config_digest = sha256_digest(config_content);

    let req = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/v2/test/repo/blobs/uploads/?digest={config_digest}"
        ))
        .body(Body::from(config_content.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // 4. Push manifest
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": config_digest,
            "size": config_content.len()
        },
        "layers": [
            {
                "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                "digest": blob_digest,
                "size": blob_content.len()
            }
        ]
    });
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();

    let req = Request::builder()
        .method(Method::PUT)
        .uri("/v2/test/repo/manifests/latest")
        .header(
            header::CONTENT_TYPE,
            "application/vnd.oci.image.manifest.v1+json",
        )
        .body(Body::from(manifest_bytes.clone()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "manifest PUT should return 201"
    );
    let manifest_digest = resp
        .headers()
        .get("Docker-Content-Digest")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    // 5. GET manifest by tag
    let req = Request::builder()
        .uri("/v2/test/repo/manifests/latest")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("Docker-Content-Digest")
            .unwrap()
            .to_str()
            .unwrap(),
        manifest_digest
    );
    assert_eq!(
        resp.headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap(),
        "application/vnd.oci.image.manifest.v1+json"
    );

    // 6. GET manifest by digest
    let req = Request::builder()
        .uri(format!("/v2/test/repo/manifests/{manifest_digest}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 7. GET blob
    let req = Request::builder()
        .uri(format!("/v2/test/repo/blobs/{blob_digest}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), blob_content);

    // 8. Tag list
    let req = Request::builder()
        .uri("/v2/test/repo/tags/list")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["tags"], serde_json::json!(["latest"]));

    // 9. Catalog
    let req = Request::builder()
        .uri("/v2/_catalog")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        json["repositories"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("test/repo"))
    );
}

/// Test chunked upload flow: POST -> PATCH -> PUT
#[tokio::test]
async fn test_chunked_upload() {
    let (app, _tmp) = setup().await;

    let blob_content = b"chunked blob data for testing";
    let blob_digest = sha256_digest(blob_content);

    // 1. POST to start upload
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v2/test/blobs/uploads/")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let upload_uuid = resp
        .headers()
        .get("Docker-Upload-UUID")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(!upload_uuid.is_empty());

    // 2. GET upload status
    let req = Request::builder()
        .uri(&location)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // 3. PATCH upload chunk
    let req = Request::builder()
        .method(Method::PATCH)
        .uri(&location)
        .body(Body::from(blob_content.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    // 4. PUT to complete
    let req = Request::builder()
        .method(Method::PUT)
        .uri(format!("{location}?digest={blob_digest}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // 5. Verify blob exists
    let req = Request::builder()
        .method(Method::HEAD)
        .uri(format!("/v2/test/blobs/{blob_digest}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Test upload cancel
#[tokio::test]
async fn test_upload_cancel() {
    let (app, _tmp) = setup().await;

    // Start upload
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v2/test/blobs/uploads/")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    // Cancel
    let req = Request::builder()
        .method(Method::DELETE)
        .uri(&location)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Verify upload is gone
    let req = Request::builder()
        .uri(&location)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// Test manifest delete
#[tokio::test]
async fn test_manifest_delete() {
    let (app, _tmp) = setup().await;

    // Push a config blob
    let config_content = b"{}";
    let config_digest = sha256_digest(config_content);
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/v2/test/blobs/uploads/?digest={config_digest}"))
        .body(Body::from(config_content.to_vec()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap();

    // Push manifest
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": config_digest,
            "size": config_content.len()
        },
        "layers": []
    });
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();

    let req = Request::builder()
        .method(Method::PUT)
        .uri("/v2/test/manifests/v1")
        .header(
            header::CONTENT_TYPE,
            "application/vnd.oci.image.manifest.v1+json",
        )
        .body(Body::from(manifest_bytes))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let digest = resp
        .headers()
        .get("Docker-Content-Digest")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    // Delete manifest by digest
    let req = Request::builder()
        .method(Method::DELETE)
        .uri(format!("/v2/test/manifests/{digest}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    // Verify manifest is gone
    let req = Request::builder()
        .uri(format!("/v2/test/manifests/{digest}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// Test blob delete
#[tokio::test]
async fn test_blob_delete() {
    let (app, _tmp) = setup().await;

    let blob_content = b"delete me";
    let blob_digest = sha256_digest(blob_content);

    // Upload blob
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/v2/test/blobs/uploads/?digest={blob_digest}"))
        .body(Body::from(blob_content.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Delete blob
    let req = Request::builder()
        .method(Method::DELETE)
        .uri(format!("/v2/test/blobs/{blob_digest}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    // Verify blob is gone
    let req = Request::builder()
        .method(Method::HEAD)
        .uri(format!("/v2/test/blobs/{blob_digest}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// Test name validation
#[tokio::test]
async fn test_invalid_repo_name() {
    let (app, _tmp) = setup().await;
    let req = Request::builder()
        .uri("/v2/INVALID/tags/list")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ── Regression tests for fixed bugs ──────────────────────────────────────────

/// P0: Monolithic upload with wrong digest should fail
#[tokio::test]
async fn test_monolithic_upload_wrong_digest() {
    let (app, _tmp) = setup().await;

    let blob_content = b"actual content";
    // Use a digest that doesn't match the content
    let wrong_digest = format!("sha256:{}", "b".repeat(64));

    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/v2/test/blobs/uploads/?digest={wrong_digest}"))
        .body(Body::from(blob_content.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "wrong digest should be rejected"
    );

    // Verify the blob was NOT stored
    let req = Request::builder()
        .method(Method::HEAD)
        .uri(format!("/v2/test/blobs/{wrong_digest}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// P0: Chunked upload complete with wrong digest should fail
#[tokio::test]
async fn test_chunked_upload_wrong_digest() {
    let (app, _tmp) = setup().await;

    let blob_content = b"chunked content";
    let wrong_digest = format!("sha256:{}", "c".repeat(64));

    // Start upload
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v2/test/blobs/uploads/")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let location = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    // PATCH chunk
    let req = Request::builder()
        .method(Method::PATCH)
        .uri(&location)
        .body(Body::from(blob_content.to_vec()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap();

    // PUT complete with wrong digest
    let req = Request::builder()
        .method(Method::PUT)
        .uri(format!("{location}?digest={wrong_digest}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "wrong digest should be rejected"
    );
}

/// P1: Multiple PATCH chunks should work with correct offsets
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn test_multi_chunk_upload() {
    let (app, _tmp) = setup().await;

    let chunk1 = b"first chunk ";
    let chunk2 = b"second chunk";
    let mut full_content = Vec::new();
    full_content.extend_from_slice(chunk1);
    full_content.extend_from_slice(chunk2);
    let blob_digest = sha256_digest(&full_content);

    // Start upload
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v2/test/blobs/uploads/")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    // PATCH first chunk
    let req = Request::builder()
        .method(Method::PATCH)
        .uri(&location)
        .body(Body::from(chunk1.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    // Verify Range header shows correct offset
    let range = resp.headers().get("Range").unwrap().to_str().unwrap();
    assert_eq!(range, format!("0-{}", chunk1.len() - 1));

    // PATCH second chunk
    let req = Request::builder()
        .method(Method::PATCH)
        .uri(&location)
        .body(Body::from(chunk2.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let range = resp.headers().get("Range").unwrap().to_str().unwrap();
    assert_eq!(range, format!("0-{}", full_content.len() - 1));

    // PUT complete
    let req = Request::builder()
        .method(Method::PUT)
        .uri(format!("{location}?digest={blob_digest}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Verify full content
    let req = Request::builder()
        .uri(format!("/v2/test/blobs/{blob_digest}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), full_content.as_slice());
}

/// P1: PUT /manifests/<digest> with body that doesn't match path digest
#[tokio::test]
async fn test_manifest_put_digest_mismatch() {
    let (app, _tmp) = setup().await;

    let manifest = serde_json::json!({"schemaVersion": 2});
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let wrong_digest = format!("sha256:{}", "d".repeat(64));

    let req = Request::builder()
        .method(Method::PUT)
        .uri(format!("/v2/test/manifests/{wrong_digest}"))
        .header(
            header::CONTENT_TYPE,
            "application/vnd.oci.image.manifest.v1+json",
        )
        .body(Body::from(manifest_bytes))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// P1: Range request with start beyond file size should return 416
#[tokio::test]
async fn test_blob_range_out_of_bounds() {
    let (app, _tmp) = setup().await;

    let blob_content = b"small blob";
    let blob_digest = sha256_digest(blob_content);

    // Upload blob
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/v2/test/blobs/uploads/?digest={blob_digest}"))
        .body(Body::from(blob_content.to_vec()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap();

    // Request range beyond size
    let req = Request::builder()
        .uri(format!("/v2/test/blobs/{blob_digest}"))
        .header(header::RANGE, "bytes=9999-10000")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
}

/// P1: Valid range request should return 206
#[tokio::test]
async fn test_blob_range_partial() {
    let (app, _tmp) = setup().await;

    let blob_content = b"hello world range test";
    let blob_digest = sha256_digest(blob_content);

    // Upload blob
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/v2/test/blobs/uploads/?digest={blob_digest}"))
        .body(Body::from(blob_content.to_vec()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap();

    // Request first 5 bytes
    let req = Request::builder()
        .uri(format!("/v2/test/blobs/{blob_digest}"))
        .header(header::RANGE, "bytes=0-4")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), b"hello");
}

/// P2: Invalid reference format should return 404 `MANIFEST_UNKNOWN` per OCI spec
#[tokio::test]
async fn test_invalid_reference_format() {
    let (app, _tmp) = setup().await;

    // Bad digest format — OCI spec: non-existent reference → 404
    let req = Request::builder()
        .uri("/v2/test/manifests/sha256:tooshort")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "malformed digest reference should be 404 MANIFEST_UNKNOWN"
    );

    // Bad tag format (starts with dot) — also 404 per OCI spec
    let req = Request::builder()
        .uri("/v2/test/manifests/.invalid-tag")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "malformed tag reference should be 404 MANIFEST_UNKNOWN"
    );
}

/// Cross-repo blob mount
#[tokio::test]
async fn test_blob_mount() {
    let (app, _tmp) = setup().await;

    let blob_content = b"shared blob for mount";
    let blob_digest = sha256_digest(blob_content);

    // Upload blob to source repo
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/v2/source/repo/blobs/uploads/?digest={blob_digest}"
        ))
        .body(Body::from(blob_content.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Mount to destination repo
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/v2/dest/repo/blobs/uploads/?mount={blob_digest}&from=source/repo"
        ))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "mount should succeed");
    assert_eq!(
        resp.headers()
            .get("Docker-Content-Digest")
            .unwrap()
            .to_str()
            .unwrap(),
        blob_digest
    );

    // Verify blob exists in dest repo
    let req = Request::builder()
        .method(Method::HEAD)
        .uri(format!("/v2/dest/repo/blobs/{blob_digest}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Mount fails when blob doesn't exist in source -> falls back to normal upload start
#[tokio::test]
async fn test_blob_mount_fallback() {
    let (app, _tmp) = setup().await;

    let fake_digest = format!("sha256:{}", "f".repeat(64));

    let req = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/v2/dest/repo/blobs/uploads/?mount={fake_digest}&from=nonexistent/repo"
        ))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::ACCEPTED,
        "should fall back to normal upload"
    );
    assert!(resp.headers().get("Location").is_some());
    assert!(resp.headers().get("Docker-Upload-UUID").is_some());
}

/// Management API: system info
#[tokio::test]
async fn test_admin_info() {
    let (app, _tmp) = setup().await;
    let req = Request::builder()
        .uri("/api/v1/info")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["version"].is_string());
}

/// Management API: GC lifecycle
#[tokio::test]
async fn test_gc_lifecycle() {
    let (app, _tmp) = setup().await;

    // Start GC
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/gc")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let job_id = json["job_id"].as_str().unwrap().to_owned();

    // Wait briefly for async GC to complete
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Query GC job status
    let req = Request::builder()
        .uri(format!("/api/v1/gc/{job_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "completed");
}

/// Management API: GC nonexistent job returns 404
#[tokio::test]
async fn test_gc_job_not_found() {
    let (app, _tmp) = setup().await;
    let req = Request::builder()
        .uri("/api/v1/gc/nonexistent-id")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// Pull count and `last_pulled_at` are updated on manifest GET.
#[tokio::test]
async fn test_pull_count_tracking() {
    let (app, _tmp) = setup().await;

    // Push a blob, config, and manifest with a tag.
    let blob = b"pull-count-test";
    let blob_digest = sha256_digest(blob);
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/v2/pullcount/repo/blobs/uploads/?digest={blob_digest}"
        ))
        .body(Body::from(blob.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let config = b"{}";
    let config_digest = sha256_digest(config);
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/v2/pullcount/repo/blobs/uploads/?digest={config_digest}"
        ))
        .body(Body::from(config.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": config_digest,
            "size": config.len()
        },
        "layers": [{
            "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
            "digest": blob_digest,
            "size": blob.len()
        }]
    });
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let req = Request::builder()
        .method(Method::PUT)
        .uri("/v2/pullcount/repo/manifests/v1")
        .header(
            header::CONTENT_TYPE,
            "application/vnd.oci.image.manifest.v1+json",
        )
        .body(Body::from(manifest_bytes.clone()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Pull the manifest twice by tag.
    for _ in 0..2 {
        let req = Request::builder()
            .uri("/v2/pullcount/repo/manifests/v1")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // Pull once by digest (should still increment pull count for the tag pointing to it).
    let manifest_digest = sha256_digest(&manifest_bytes);
    let req = Request::builder()
        .uri(format!("/v2/pullcount/repo/manifests/{manifest_digest}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify via tag list that pulls were counted (tag still exists).
    let req = Request::builder()
        .uri("/v2/pullcount/repo/tags/list")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["tags"], serde_json::json!(["v1"]));
}

/// Auth middleware returns 401 with WWW-Authenticate when auth is enabled.
#[tokio::test]
async fn test_auth_challenge_when_enabled() {
    let tmp_dir = TempDir::new().unwrap();
    let storage_dir = tmp_dir.path().join("storage");
    let db_path = tmp_dir.path().join("test.db");

    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let db = Database::connect(ConnectOptions::new(&db_url))
        .await
        .unwrap();
    db.execute_unprepared("PRAGMA foreign_keys = ON")
        .await
        .unwrap();
    db.execute_unprepared("PRAGMA journal_mode = WAL")
        .await
        .unwrap();
    migration::Migrator::up(&db, None).await.unwrap();

    let storage = Arc::new(FilesystemStorage::new(storage_dir).await.unwrap());
    let registry = Arc::new(Registry::new(db, storage));
    let state = AppState {
        registry,
        auth_config: distribution::config::AuthConfig {
            enabled: true,
            anonymous_pull: false,
            jwt_secret: None,
            basic_credentials: Vec::new(),
        },
    };
    let app = router(state);

    // Unauthenticated request should get 401.
    let req = Request::builder().uri("/v2/").body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(resp.headers().get("WWW-Authenticate").is_some());

    // With a Bearer token, request should succeed.
    let req = Request::builder()
        .uri("/v2/")
        .header("Authorization", "Bearer sometoken")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Auth middleware allows anonymous GET/HEAD when `anonymous_pull` is enabled.
#[tokio::test]
async fn test_auth_anonymous_pull() {
    let tmp_dir = TempDir::new().unwrap();
    let storage_dir = tmp_dir.path().join("storage");
    let db_path = tmp_dir.path().join("test.db");

    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let db = Database::connect(ConnectOptions::new(&db_url))
        .await
        .unwrap();
    db.execute_unprepared("PRAGMA foreign_keys = ON")
        .await
        .unwrap();
    db.execute_unprepared("PRAGMA journal_mode = WAL")
        .await
        .unwrap();
    migration::Migrator::up(&db, None).await.unwrap();

    let storage = Arc::new(FilesystemStorage::new(storage_dir).await.unwrap());
    let registry = Arc::new(Registry::new(db, storage));
    let state = AppState {
        registry,
        auth_config: distribution::config::AuthConfig {
            enabled: true,
            anonymous_pull: true,
            jwt_secret: None,
            basic_credentials: Vec::new(),
        },
    };
    let app = router(state);

    // Anonymous GET on /v2/ should succeed (anonymous_pull allows data-plane reads).
    let req = Request::builder().uri("/v2/").body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Anonymous POST (write) should be rejected.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v2/test/repo/blobs/uploads/")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── PLAN 1.13 Directed regression tests ──────────────────────────────────────

/// Upload session repo isolation: using repo A's session on repo B path
/// should return 404 `BLOB_UPLOAD_UNKNOWN`.
#[tokio::test]
async fn test_upload_session_repo_isolation() {
    let (app, _tmp) = setup().await;

    // Start an upload session on repo-a.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v2/repo-a/blobs/uploads/")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    // Extract session ID from location: /v2/repo-a/blobs/uploads/<uuid>
    let session_id = location.rsplit('/').next().unwrap();

    // Try to GET the upload via repo-b path -> should 404.
    let req = Request::builder()
        .uri(format!("/v2/repo-b/blobs/uploads/{session_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Try to PATCH via repo-b path -> should 404.
    let req = Request::builder()
        .method(Method::PATCH)
        .uri(format!("/v2/repo-b/blobs/uploads/{session_id}"))
        .body(Body::from("data"))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Try to DELETE via repo-b path -> should 404.
    let req = Request::builder()
        .method(Method::DELETE)
        .uri(format!("/v2/repo-b/blobs/uploads/{session_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Original repo-a path still works.
    let req = Request::builder()
        .uri(&location)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

/// Manifest PUT with missing config blob should return `MANIFEST_BLOB_UNKNOWN`.
#[tokio::test]
async fn test_manifest_put_missing_config_blob() {
    let (app, _tmp) = setup().await;

    // Push only the layer blob, NOT the config blob.
    let layer = b"layer-data";
    let layer_digest = sha256_digest(layer);
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/v2/test/ref-check/blobs/uploads/?digest={layer_digest}"
        ))
        .body(Body::from(layer.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let fake_config_digest =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000";
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": fake_config_digest,
            "size": 2
        },
        "layers": [{
            "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
            "digest": layer_digest,
            "size": layer.len()
        }]
    });
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let req = Request::builder()
        .method(Method::PUT)
        .uri("/v2/test/ref-check/manifests/latest")
        .header(
            header::CONTENT_TYPE,
            "application/vnd.oci.image.manifest.v1+json",
        )
        .body(Body::from(manifest_bytes))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["errors"][0]["code"], "MANIFEST_BLOB_UNKNOWN");
}

/// Manifest PUT with missing layer blob should return `MANIFEST_BLOB_UNKNOWN`.
#[tokio::test]
async fn test_manifest_put_missing_layer_blob() {
    let (app, _tmp) = setup().await;

    // Push only the config blob, NOT the layer blob.
    let config = b"{}";
    let config_digest = sha256_digest(config);
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/v2/test/ref-check2/blobs/uploads/?digest={config_digest}"
        ))
        .body(Body::from(config.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let fake_layer_digest =
        "sha256:1111111111111111111111111111111111111111111111111111111111111111";
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": config_digest,
            "size": config.len()
        },
        "layers": [{
            "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
            "digest": fake_layer_digest,
            "size": 10
        }]
    });
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let req = Request::builder()
        .method(Method::PUT)
        .uri("/v2/test/ref-check2/manifests/v1")
        .header(
            header::CONTENT_TYPE,
            "application/vnd.oci.image.manifest.v1+json",
        )
        .body(Body::from(manifest_bytes))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["errors"][0]["code"], "MANIFEST_BLOB_UNKNOWN");
}

/// Index PUT with missing sub-manifest should return `MANIFEST_BLOB_UNKNOWN`.
#[tokio::test]
async fn test_index_put_missing_sub_manifest() {
    let (app, _tmp) = setup().await;

    let fake_sub_digest = "sha256:2222222222222222222222222222222222222222222222222222222222222222";
    let index = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": [{
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "digest": fake_sub_digest,
            "size": 100,
            "platform": { "architecture": "amd64", "os": "linux" }
        }]
    });
    let index_bytes = serde_json::to_vec(&index).unwrap();

    // Ensure the repository exists.
    let blob = b"x";
    let blob_digest = sha256_digest(blob);
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/v2/test/index-check/blobs/uploads/?digest={blob_digest}"
        ))
        .body(Body::from(blob.to_vec()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = Request::builder()
        .method(Method::PUT)
        .uri("/v2/test/index-check/manifests/multi")
        .header(
            header::CONTENT_TYPE,
            "application/vnd.oci.image.index.v1+json",
        )
        .body(Body::from(index_bytes))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["errors"][0]["code"], "MANIFEST_BLOB_UNKNOWN");
}

/// Cancel upload should remove the temp file from storage.
#[tokio::test]
async fn test_upload_cancel_cleans_tmp_file() {
    let tmp_dir = TempDir::new().unwrap();
    let storage_dir = tmp_dir.path().join("storage");
    let db_path = tmp_dir.path().join("test.db");

    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let db = Database::connect(ConnectOptions::new(&db_url))
        .await
        .unwrap();
    db.execute_unprepared("PRAGMA foreign_keys = ON")
        .await
        .unwrap();
    db.execute_unprepared("PRAGMA journal_mode = WAL")
        .await
        .unwrap();
    migration::Migrator::up(&db, None).await.unwrap();

    let storage = Arc::new(FilesystemStorage::new(storage_dir.clone()).await.unwrap());
    let registry = Arc::new(Registry::new(db, storage));
    let state = AppState {
        registry,
        auth_config: distribution::config::AuthConfig::default(),
    };
    let app = router(state);

    // Start upload.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v2/test/cancel-clean/blobs/uploads/")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let session_id = location.rsplit('/').next().unwrap();

    // Verify temp file exists.
    let upload_file = storage_dir.join("uploads").join(session_id);
    assert!(
        upload_file.exists(),
        "upload temp file should exist before cancel"
    );

    // Cancel.
    let req = Request::builder()
        .method(Method::DELETE)
        .uri(&location)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Verify temp file is removed.
    assert!(
        !upload_file.exists(),
        "upload temp file should be removed after cancel"
    );
}

/// `startup_cleanup` removes orphan upload files that have no DB session.
#[tokio::test]
async fn test_startup_cleanup_orphan_uploads() {
    let tmp_dir = TempDir::new().unwrap();
    let storage_dir = tmp_dir.path().join("storage");
    let db_path = tmp_dir.path().join("test.db");

    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let db = Database::connect(ConnectOptions::new(&db_url))
        .await
        .unwrap();
    db.execute_unprepared("PRAGMA foreign_keys = ON")
        .await
        .unwrap();
    db.execute_unprepared("PRAGMA journal_mode = WAL")
        .await
        .unwrap();
    migration::Migrator::up(&db, None).await.unwrap();

    let storage = Arc::new(FilesystemStorage::new(storage_dir.clone()).await.unwrap());

    // Create an orphan upload file (no matching DB session).
    let orphan_id = uuid::Uuid::new_v4().to_string();
    tokio::fs::write(storage_dir.join("uploads").join(&orphan_id), b"orphan")
        .await
        .unwrap();

    // Create an orphan tmp file.
    tokio::fs::write(storage_dir.join("tmp").join("leftover"), b"tmp-orphan")
        .await
        .unwrap();

    assert!(storage_dir.join("uploads").join(&orphan_id).exists());
    assert!(storage_dir.join("tmp").join("leftover").exists());

    let registry = Arc::new(Registry::new(db, storage));
    registry.startup_cleanup().await.unwrap();

    // Both should be removed.
    assert!(
        !storage_dir.join("uploads").join(&orphan_id).exists(),
        "orphan upload file should be removed"
    );
    assert!(
        !storage_dir.join("tmp").join("leftover").exists(),
        "orphan tmp file should be removed"
    );
}

/// JWT auth: valid HS256 token should authenticate.
#[tokio::test]
async fn test_jwt_auth_valid_token() {
    let tmp_dir = TempDir::new().unwrap();
    let storage_dir = tmp_dir.path().join("storage");
    let db_path = tmp_dir.path().join("test.db");

    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let db = Database::connect(ConnectOptions::new(&db_url))
        .await
        .unwrap();
    db.execute_unprepared("PRAGMA foreign_keys = ON")
        .await
        .unwrap();
    db.execute_unprepared("PRAGMA journal_mode = WAL")
        .await
        .unwrap();
    migration::Migrator::up(&db, None).await.unwrap();

    let storage = Arc::new(FilesystemStorage::new(storage_dir).await.unwrap());
    let registry = Arc::new(Registry::new(db, storage));

    let secret = "test-jwt-secret-key-1234567890";
    let state = AppState {
        registry,
        auth_config: distribution::config::AuthConfig {
            enabled: true,
            anonymous_pull: false,
            jwt_secret: Some(secret.to_owned()),
            basic_credentials: Vec::new(),
        },
    };
    let app = router(state);

    // Create a valid JWT.
    let claims = serde_json::json!({ "tenant_id": "_default" });
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap();

    // Valid JWT -> 200.
    let req = Request::builder()
        .uri("/v2/")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Invalid JWT -> 401.
    let req = Request::builder()
        .uri("/v2/")
        .header("Authorization", "Bearer invalid.jwt.token")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // No token -> 401.
    let req = Request::builder().uri("/v2/").body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Basic auth: correct credentials should authenticate, wrong should reject.
#[tokio::test]
async fn test_basic_auth_validation() {
    let tmp_dir = TempDir::new().unwrap();
    let storage_dir = tmp_dir.path().join("storage");
    let db_path = tmp_dir.path().join("test.db");

    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let db = Database::connect(ConnectOptions::new(&db_url))
        .await
        .unwrap();
    db.execute_unprepared("PRAGMA foreign_keys = ON")
        .await
        .unwrap();
    db.execute_unprepared("PRAGMA journal_mode = WAL")
        .await
        .unwrap();
    migration::Migrator::up(&db, None).await.unwrap();

    let storage = Arc::new(FilesystemStorage::new(storage_dir).await.unwrap());
    let registry = Arc::new(Registry::new(db, storage));
    let state = AppState {
        registry,
        auth_config: distribution::config::AuthConfig {
            enabled: true,
            anonymous_pull: false,
            jwt_secret: None,
            basic_credentials: vec!["admin:secret123".to_owned()],
        },
    };
    let app = router(state);

    // Correct Basic auth -> 200.
    let encoded = base64::engine::general_purpose::STANDARD.encode("admin:secret123");
    let req = Request::builder()
        .uri("/v2/")
        .header("Authorization", format!("Basic {encoded}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Wrong password -> 401.
    let bad_encoded = base64::engine::general_purpose::STANDARD.encode("admin:wrong");
    let req = Request::builder()
        .uri("/v2/")
        .header("Authorization", format!("Basic {bad_encoded}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
