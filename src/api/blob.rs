use crate::error::RegistryError;
use crate::registry::Registry;
use crate::types::{Digest, RepoName, TenantId};
use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio_util::io::ReaderStream;

pub async fn get_blob(
    registry: &Registry,
    tenant_id: &TenantId,
    name: &RepoName,
    digest: &Digest,
    headers: &HeaderMap,
) -> Result<Response, RegistryError> {
    if let Some(range_val) = headers.get(header::RANGE) {
        if let Some((start, end)) = range_val.to_str().ok().and_then(parse_range_header) {
            let total_size = registry.blob_head(tenant_id, name, digest).await?;
            if total_size == 0 || start >= total_size {
                return Ok(Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{total_size}"))
                    .body(Body::empty())
                    .unwrap()
                    .into_response());
            }
            let end = end.map_or(total_size - 1, |e| e.min(total_size - 1));
            if start > end {
                return Ok(Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{total_size}"))
                    .body(Body::empty())
                    .unwrap()
                    .into_response());
            }
            let length = end - start + 1;
            let reader = registry
                .get_blob_range(tenant_id, name, digest, start, length)
                .await?;
            let stream = ReaderStream::new(reader);
            return Ok(Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(header::CONTENT_LENGTH, length)
                .header(
                    header::CONTENT_RANGE,
                    format!("bytes {start}-{end}/{total_size}"),
                )
                .header("Docker-Content-Digest", digest.to_string())
                .header(header::ACCEPT_RANGES, "bytes")
                .body(Body::from_stream(stream))
                .unwrap()
                .into_response());
        }
    }

    let (reader, size) = registry.get_blob(tenant_id, name, digest).await?;
    let stream = ReaderStream::new(reader);
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, size)
        .header("Docker-Content-Digest", digest.to_string())
        .header(header::ACCEPT_RANGES, "bytes")
        .body(Body::from_stream(stream))
        .unwrap()
        .into_response())
}

pub async fn head_blob(
    registry: &Registry,
    tenant_id: &TenantId,
    name: &RepoName,
    digest: &Digest,
) -> Result<Response, RegistryError> {
    let size = registry.blob_head(tenant_id, name, digest).await?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, size)
        .header("Docker-Content-Digest", digest.to_string())
        .header(header::ACCEPT_RANGES, "bytes")
        .body(Body::empty())
        .unwrap()
        .into_response())
}

pub async fn delete_blob(
    registry: &Registry,
    tenant_id: &TenantId,
    name: &RepoName,
    digest: &Digest,
) -> Result<Response, RegistryError> {
    registry.delete_blob(tenant_id, name, digest).await?;
    Ok(StatusCode::ACCEPTED.into_response())
}

fn parse_range_header(s: &str) -> Option<(u64, Option<u64>)> {
    let bytes = s.strip_prefix("bytes=")?;
    let (start_str, end_str) = bytes.split_once('-')?;
    let start: u64 = start_str.parse().ok()?;
    let end: Option<u64> = if end_str.is_empty() {
        None
    } else {
        end_str.parse().ok()
    };
    Some((start, end))
}
