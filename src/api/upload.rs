use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::TryStreamExt;
use tokio_util::io::StreamReader;

use crate::error::{OciError, RegistryError};
use crate::registry::Registry;
use crate::types::{Digest, RepoName, TenantId, UploadId};

fn upload_location(name: &RepoName, upload_id: &UploadId) -> String {
    format!("/v2/{name}/blobs/uploads/{upload_id}")
}

fn range_header(next_offset: u64) -> String {
    if next_offset == 0 {
        "0-0".to_owned()
    } else {
        format!("0-{}", next_offset - 1)
    }
}

fn body_to_reader(req: Request<Body>) -> Box<dyn tokio::io::AsyncRead + Send + Unpin> {
    let body_stream = req
        .into_body()
        .into_data_stream()
        .map_err(std::io::Error::other);
    Box::new(StreamReader::new(body_stream))
}

pub async fn start_upload(
    registry: &Registry,
    tenant_id: &TenantId,
    name: &RepoName,
    digest: Option<&Digest>,
    mount_digest: Option<&Digest>,
    from_repo: Option<&RepoName>,
    req: Request<Body>,
) -> Result<Response, RegistryError> {
    // Cross-repo mount
    if let (Some(mount_digest), Some(from)) = (mount_digest, from_repo) {
        if registry
            .mount_blob(tenant_id, name, from, mount_digest)
            .await?
        {
            return Ok((
                StatusCode::CREATED,
                [
                    ("Location", format!("/v2/{name}/blobs/{mount_digest}")),
                    ("Docker-Content-Digest", mount_digest.to_string()),
                ],
            )
                .into_response());
        }
        // Mount failed, fall through to normal upload
    }

    // Monolithic upload: ?digest=<d>
    if let Some(digest) = digest {
        let reader = body_to_reader(req);
        registry
            .monolithic_upload(tenant_id, name, digest, reader)
            .await?;

        return Ok((
            StatusCode::CREATED,
            [
                ("Location", format!("/v2/{name}/blobs/{digest}")),
                ("Docker-Content-Digest", digest.to_string()),
            ],
        )
            .into_response());
    }

    // Cross-repo mount (stage 1): fall through to normal upload start.
    let upload_id = registry.start_upload(tenant_id, name).await?;

    Ok((
        StatusCode::ACCEPTED,
        [
            ("Location", upload_location(name, &upload_id)),
            ("Range", "0-0".to_owned()),
            ("Docker-Upload-UUID", upload_id.to_string()),
        ],
    )
        .into_response())
}

pub async fn get_upload(
    registry: &Registry,
    tenant_id: &TenantId,
    name: &RepoName,
    upload_id: &UploadId,
) -> Result<Response, RegistryError> {
    let next_offset = registry
        .get_upload_status(tenant_id, name, upload_id)
        .await?;

    Ok((
        StatusCode::NO_CONTENT,
        [
            ("Location", upload_location(name, upload_id)),
            ("Range", range_header(next_offset)),
            ("Docker-Upload-UUID", upload_id.to_string()),
        ],
    )
        .into_response())
}

pub async fn patch_upload(
    registry: &Registry,
    tenant_id: &TenantId,
    name: &RepoName,
    upload_id: &UploadId,
    req: Request<Body>,
) -> Result<Response, RegistryError> {
    let offset = if let Some(content_range) = req.headers().get("Content-Range") {
        let range_str = content_range.to_str().unwrap_or("");
        parse_content_range_start(range_str).ok_or_else(|| {
            OciError::blob_upload_invalid(format!("invalid Content-Range: {range_str}"))
        })?
    } else {
        registry
            .get_upload_status(tenant_id, name, upload_id)
            .await?
    };

    let reader = body_to_reader(req);
    let new_offset = registry
        .write_upload_chunk(tenant_id, name, upload_id, reader, offset)
        .await?;

    Ok((
        StatusCode::ACCEPTED,
        [
            ("Location", upload_location(name, upload_id)),
            ("Range", range_header(new_offset)),
            ("Docker-Upload-UUID", upload_id.to_string()),
        ],
    )
        .into_response())
}

pub async fn complete_upload(
    registry: &Registry,
    tenant_id: &TenantId,
    name: &RepoName,
    upload_id: &UploadId,
    digest: &Digest,
    req: Request<Body>,
) -> Result<Response, RegistryError> {
    // If body is non-empty, stream a final chunk before completing.
    // Use the streaming reader (not to_bytes) to avoid buffering large PUTs.
    let offset = registry
        .get_upload_status(tenant_id, name, upload_id)
        .await?;
    let reader = body_to_reader(req);
    registry
        .write_upload_chunk(tenant_id, name, upload_id, reader, offset)
        .await?;

    registry
        .complete_upload(tenant_id, name, upload_id, digest)
        .await?;

    Ok((
        StatusCode::CREATED,
        [
            ("Location", format!("/v2/{name}/blobs/{digest}")),
            ("Docker-Content-Digest", digest.to_string()),
        ],
    )
        .into_response())
}

pub async fn cancel_upload(
    registry: &Registry,
    tenant_id: &TenantId,
    name: &RepoName,
    upload_id: &UploadId,
) -> Result<Response, RegistryError> {
    registry.cancel_upload(tenant_id, name, upload_id).await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Parse the start offset from a Content-Range header like "0-99" or "100-199".
fn parse_content_range_start(s: &str) -> Option<u64> {
    s.split('-').next()?.parse().ok()
}
