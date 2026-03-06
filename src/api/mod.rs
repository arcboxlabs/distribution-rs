pub mod admin;
pub mod blob;
pub mod catalog;
pub mod manifest;
pub mod referrer;
pub mod tag;
pub mod upload;
pub mod v2;

use crate::auth::middleware::{AuthInfo, auth_middleware};
use crate::config::AuthConfig;
use crate::error::OciError;
use crate::registry::Registry;
use crate::types::{Digest, Reference, RepoName, UploadId};
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Registry>,
    pub auth_config: AuthConfig,
}

pub fn router(state: AppState) -> Router {
    let health_routes = Router::new()
        .route("/healthz", axum::routing::get(healthz))
        .route("/readyz", axum::routing::get(readyz));

    let admin_routes = Router::new()
        .route("/api/v1/info", axum::routing::get(admin::get_info))
        .route("/api/v1/gc", axum::routing::post(admin::start_gc))
        .route("/api/v1/gc/{job_id}", axum::routing::get(admin::get_gc_job))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    let api_routes = Router::new()
        .route("/v2/", axum::routing::get(v2::version_check))
        .fallback(v2_fallback)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    api_routes
        .merge(health_routes)
        .merge(admin_routes)
        .with_state(state)
}

async fn healthz() -> &'static str {
    "OK"
}

async fn readyz(State(state): State<AppState>) -> Result<&'static str, StatusCode> {
    state
        .registry
        .health_check()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok("OK")
}

#[allow(clippy::too_many_lines)]
async fn v2_fallback(State(state): State<AppState>, req: Request<Body>) -> Response {
    let path = req.uri().path().to_owned();
    let query = req.uri().query().unwrap_or("").to_owned();
    let method = req.method().clone();
    let headers = req.headers().clone();

    let Some(rest) = path.strip_prefix("/v2/") else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let auth_info = req
        .extensions()
        .get::<AuthInfo>()
        .cloned()
        .unwrap_or_else(AuthInfo::anonymous);
    let tenant_id = auth_info.tenant_id;

    // /v2/<name>/manifests/<reference>
    if let Some((name_str, reference_str)) = split_last_segment(rest, "/manifests/") {
        let Ok(name) = name_str.parse::<RepoName>() else {
            return OciError::name_invalid(name_str).into_response();
        };
        let Ok(reference) = reference_str.parse::<Reference>() else {
            return OciError::manifest_unknown(reference_str).into_response();
        };
        return match method {
            Method::GET => manifest::get_manifest(&state.registry, &tenant_id, &name, &reference)
                .await
                .into_response(),
            Method::HEAD => manifest::head_manifest(&state.registry, &tenant_id, &name, &reference)
                .await
                .into_response(),
            Method::PUT => {
                manifest::put_manifest(&state.registry, &tenant_id, &name, &reference, req)
                    .await
                    .into_response()
            }
            Method::DELETE => {
                let Reference::Digest(digest) = reference else {
                    return OciError::unsupported().into_response();
                };
                manifest::delete_manifest(&state.registry, &tenant_id, &name, &digest)
                    .await
                    .into_response()
            }
            _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        };
    }

    // /v2/<name>/blobs/uploads/<session_id> (must check before /blobs/)
    if let Some((name_str, session_id_str)) = split_last_segment(rest, "/blobs/uploads/") {
        let Ok(name) = name_str.parse::<RepoName>() else {
            return OciError::name_invalid(name_str).into_response();
        };
        let Ok(session_id) = session_id_str.parse::<UploadId>() else {
            return OciError::blob_upload_unknown().into_response();
        };
        return match method {
            Method::GET => upload::get_upload(&state.registry, &tenant_id, &name, &session_id)
                .await
                .into_response(),
            Method::PATCH => {
                upload::patch_upload(&state.registry, &tenant_id, &name, &session_id, req)
                    .await
                    .into_response()
            }
            Method::PUT => {
                let digest =
                    parse_query_param(&query, "digest").and_then(|s| s.parse::<Digest>().ok());
                let Some(digest) = digest else {
                    return OciError::digest_invalid("missing or invalid digest query param")
                        .into_response();
                };
                upload::complete_upload(
                    &state.registry,
                    &tenant_id,
                    &name,
                    &session_id,
                    &digest,
                    req,
                )
                .await
                .into_response()
            }
            Method::DELETE => {
                upload::cancel_upload(&state.registry, &tenant_id, &name, &session_id)
                    .await
                    .into_response()
            }
            _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        };
    }

    // /v2/<name>/blobs/uploads/ or /v2/<name>/blobs/uploads (POST to initiate)
    if let Some(name_str) = rest
        .strip_suffix("/blobs/uploads/")
        .or_else(|| rest.strip_suffix("/blobs/uploads"))
    {
        let Ok(name) = name_str.parse::<RepoName>() else {
            return OciError::name_invalid(name_str).into_response();
        };
        return match method {
            Method::POST => {
                let digest_param =
                    parse_query_param(&query, "digest").and_then(|s| s.parse::<Digest>().ok());
                let mount_digest =
                    parse_query_param(&query, "mount").and_then(|s| s.parse::<Digest>().ok());
                let from_repo =
                    parse_query_param(&query, "from").and_then(|s| s.parse::<RepoName>().ok());
                upload::start_upload(
                    &state.registry,
                    &tenant_id,
                    &name,
                    digest_param.as_ref(),
                    mount_digest.as_ref(),
                    from_repo.as_ref(),
                    req,
                )
                .await
                .into_response()
            }
            _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        };
    }

    // /v2/<name>/blobs/<digest>
    if let Some((name_str, digest_str)) = split_last_segment(rest, "/blobs/") {
        let Ok(name) = name_str.parse::<RepoName>() else {
            return OciError::name_invalid(name_str).into_response();
        };
        let Ok(digest) = digest_str.parse::<Digest>() else {
            return OciError::digest_invalid(digest_str).into_response();
        };
        return match method {
            Method::GET => blob::get_blob(&state.registry, &tenant_id, &name, &digest, &headers)
                .await
                .into_response(),
            Method::HEAD => blob::head_blob(&state.registry, &tenant_id, &name, &digest)
                .await
                .into_response(),
            Method::DELETE => blob::delete_blob(&state.registry, &tenant_id, &name, &digest)
                .await
                .into_response(),
            _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        };
    }

    // /v2/<name>/tags/list
    if let Some(name_str) = rest.strip_suffix("/tags/list") {
        let Ok(name) = name_str.parse::<RepoName>() else {
            return OciError::name_invalid(name_str).into_response();
        };
        return match method {
            Method::GET => {
                let n = parse_query_param(&query, "n").and_then(|s| s.parse::<u64>().ok());
                let last = parse_query_param(&query, "last");
                tag::list_tags(&state.registry, &tenant_id, &name, n, last.as_deref())
                    .await
                    .into_response()
            }
            _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        };
    }

    // /v2/<name>/referrers/<digest>
    if let Some((name_str, digest_str)) = split_last_segment(rest, "/referrers/") {
        let Ok(name) = name_str.parse::<RepoName>() else {
            return OciError::name_invalid(name_str).into_response();
        };
        let Ok(digest) = digest_str.parse::<Digest>() else {
            return OciError::digest_invalid(digest_str).into_response();
        };
        return match method {
            Method::GET => {
                let artifact_type = parse_query_param(&query, "artifactType");
                referrer::get_referrers(
                    &state.registry,
                    &tenant_id,
                    &name,
                    &digest,
                    artifact_type.as_deref(),
                )
                .await
                .into_response()
            }
            _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        };
    }

    // /v2/_catalog
    if rest == "_catalog" {
        return match method {
            Method::GET => {
                let n = parse_query_param(&query, "n").and_then(|s| s.parse::<u64>().ok());
                let last = parse_query_param(&query, "last");
                catalog::list_repositories(&state.registry, &tenant_id, n, last.as_deref())
                    .await
                    .into_response()
            }
            _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        };
    }

    StatusCode::NOT_FOUND.into_response()
}

/// Split `path` on the *last* occurrence of `needle`, returning
/// `(before, after)`. Useful for extracting `(name, reference)` from
/// `library/alpine/manifests/latest`.
fn split_last_segment<'a>(path: &'a str, needle: &str) -> Option<(&'a str, &'a str)> {
    let idx = path.rfind(needle)?;
    let before = &path[..idx];
    let after = &path[idx + needle.len()..];
    if before.is_empty() || after.is_empty() {
        return None;
    }
    Some((before, after))
}

fn parse_query_param(query: &str, key: &str) -> Option<String> {
    form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}
