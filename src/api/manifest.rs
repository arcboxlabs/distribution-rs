use crate::error::{OciError, RegistryError};
use crate::registry::Registry;
use crate::types::{Digest, Reference, RepoName, TenantId};
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Response};

pub async fn get_manifest(
    registry: &Registry,
    tenant_id: &TenantId,
    name: &RepoName,
    reference: &Reference,
) -> Result<Response, RegistryError> {
    let (bytes, media_type, digest) = registry.get_manifest(tenant_id, name, reference).await?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &media_type)
        .header(header::CONTENT_LENGTH, bytes.len())
        .header("Docker-Content-Digest", digest.to_string())
        .body(Body::from(bytes))
        .unwrap()
        .into_response())
}

pub async fn head_manifest(
    registry: &Registry,
    tenant_id: &TenantId,
    name: &RepoName,
    reference: &Reference,
) -> Result<Response, RegistryError> {
    let (media_type, digest, size) = registry.head_manifest(tenant_id, name, reference).await?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &media_type)
        .header(header::CONTENT_LENGTH, size)
        .header("Docker-Content-Digest", digest.to_string())
        .body(Body::empty())
        .unwrap()
        .into_response())
}

pub async fn put_manifest(
    registry: &Registry,
    tenant_id: &TenantId,
    name: &RepoName,
    reference: &Reference,
    req: Request<Body>,
) -> Result<Response, RegistryError> {
    let media_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/vnd.oci.image.manifest.v1+json")
        .to_owned();
    let body_bytes = axum::body::to_bytes(req.into_body(), 10 * 1024 * 1024)
        .await
        .map_err(|e| OciError::manifest_invalid(e.to_string()))?;
    let (digest, subject_digest) = registry
        .put_manifest(tenant_id, name, reference, &media_type, &body_bytes)
        .await?;
    let location = format!("/v2/{name}/manifests/{digest}");
    let mut builder = Response::builder()
        .status(StatusCode::CREATED)
        .header(header::LOCATION, &location)
        .header("Docker-Content-Digest", digest.to_string());
    if let Some(subject) = subject_digest {
        builder = builder.header("OCI-Subject", subject);
    }
    Ok(builder.body(Body::empty()).unwrap().into_response())
}

pub async fn delete_manifest(
    registry: &Registry,
    tenant_id: &TenantId,
    name: &RepoName,
    digest: &Digest,
) -> Result<Response, RegistryError> {
    registry.delete_manifest(tenant_id, name, digest).await?;
    Ok(StatusCode::ACCEPTED.into_response())
}
