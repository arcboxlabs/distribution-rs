use crate::error::RegistryError;
use crate::registry::Registry;
use crate::types::TenantId;
use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Serialize)]
struct CatalogResponse {
    repositories: Vec<String>,
}

pub async fn list_repositories(
    registry: &Registry,
    tenant_id: &TenantId,
    n: Option<u64>,
    last: Option<&str>,
) -> Result<Response, RegistryError> {
    let (repos, has_more) = registry.list_repositories(tenant_id, n, last).await?;
    let resp = CatalogResponse {
        repositories: repos.clone(),
    };
    let json = serde_json::to_vec(&resp).unwrap();
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json");
    if has_more {
        if let Some(last_repo) = repos.last() {
            let limit = n.unwrap_or(100);
            builder = builder.header(
                header::LINK,
                format!("</v2/_catalog?n={limit}&last={last_repo}>; rel=\"next\""),
            );
        }
    }
    Ok(builder.body(Body::from(json)).unwrap().into_response())
}
