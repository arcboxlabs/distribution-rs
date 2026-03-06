use crate::error::RegistryError;
use crate::registry::Registry;
use crate::types::{Digest, RepoName, TenantId};
use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

const OCI_IMAGE_INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";

#[derive(Serialize)]
struct ReferrerDescriptor {
    #[serde(rename = "mediaType")]
    media_type: String,
    digest: String,
    size: i64,
    #[serde(rename = "artifactType", skip_serializing_if = "Option::is_none")]
    artifact_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    annotations: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct ReferrersResponse {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    #[serde(rename = "mediaType")]
    media_type: &'static str,
    manifests: Vec<ReferrerDescriptor>,
}

pub async fn get_referrers(
    registry: &Registry,
    tenant_id: &TenantId,
    name: &RepoName,
    digest: &Digest,
    artifact_type_filter: Option<&str>,
) -> Result<Response, RegistryError> {
    let rows = registry
        .list_referrers(tenant_id, name, digest, artifact_type_filter)
        .await?;

    let manifests: Vec<ReferrerDescriptor> = rows
        .into_iter()
        .map(|r| {
            let annotations = r.annotations.and_then(|s| serde_json::from_str(&s).ok());
            ReferrerDescriptor {
                media_type: r.media_type,
                digest: r.referrer_digest,
                size: r.size,
                artifact_type: r.artifact_type,
                annotations,
            }
        })
        .collect();

    let resp = ReferrersResponse {
        schema_version: 2,
        media_type: OCI_IMAGE_INDEX_MEDIA_TYPE,
        manifests,
    };
    let json = serde_json::to_vec(&resp).unwrap();

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, OCI_IMAGE_INDEX_MEDIA_TYPE);
    if artifact_type_filter.is_some() {
        builder = builder.header("OCI-Filters-Applied", "artifactType");
    }
    Ok(builder.body(Body::from(json)).unwrap().into_response())
}
