use crate::error::RegistryError;
use crate::registry::Registry;
use crate::types::{RepoName, TenantId};
use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Serialize)]
struct TagListResponse {
    name: String,
    tags: Vec<String>,
}

pub async fn list_tags(
    registry: &Registry,
    tenant_id: &TenantId,
    name: &RepoName,
    n: Option<u64>,
    last: Option<&str>,
) -> Result<Response, RegistryError> {
    let (tags, has_more) = registry.list_tags(tenant_id, name, n, last).await?;
    let resp = TagListResponse {
        name: name.to_string(),
        tags: tags.clone(),
    };
    let json = serde_json::to_vec(&resp).unwrap();
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json");
    if has_more {
        if let Some(last_tag) = tags.last() {
            let limit = n.unwrap_or(100);
            builder = builder.header(
                header::LINK,
                format!("</v2/{name}/tags/list?n={limit}&last={last_tag}>; rel=\"next\""),
            );
        }
    }
    Ok(builder.body(Body::from(json)).unwrap().into_response())
}
