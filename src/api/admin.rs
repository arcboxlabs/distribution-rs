use crate::api::AppState;
use crate::error::RegistryError;
use axum::body::Body;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

// GET /api/v1/info
#[derive(Serialize)]
struct InfoResponse {
    version: &'static str,
    storage_backend: &'static str,
    database: &'static str,
}

pub async fn get_info() -> impl IntoResponse {
    let resp = InfoResponse {
        version: env!("CARGO_PKG_VERSION"),
        storage_backend: "filesystem",
        database: "sqlite",
    };
    axum::Json(resp)
}

// POST /api/v1/gc -> 202 + job_id
#[derive(Serialize)]
struct GcStartResponse {
    job_id: String,
}

pub async fn start_gc(State(state): State<AppState>) -> Result<Response, RegistryError> {
    let job_id = match state.registry.start_gc().await {
        Ok(id) => id,
        Err(RegistryError::Oci(ref e)) if e.code == crate::error::ErrorCode::Denied => {
            return Ok(StatusCode::CONFLICT.into_response());
        }
        Err(e) => return Err(e),
    };

    let registry = state.registry.clone();
    let jid = job_id.clone();
    tokio::spawn(async move {
        if let Err(e) = registry.run_gc(&jid).await {
            tracing::error!("GC job {jid} failed: {e}");
            let _ = registry.mark_gc_failed(&jid).await;
        }
    });

    let resp = GcStartResponse { job_id };
    let json = serde_json::to_vec(&resp).unwrap();
    Ok(Response::builder()
        .status(StatusCode::ACCEPTED)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json))
        .unwrap()
        .into_response())
}

// GET /api/v1/gc/:job_id
#[derive(Serialize)]
struct GcJobResponse {
    id: String,
    status: String,
    started_at: Option<String>,
    completed_at: Option<String>,
    stats: Option<serde_json::Value>,
    created_at: String,
}

#[allow(clippy::similar_names)]
pub async fn get_gc_job(
    State(state): State<AppState>,
    axum::extract::Path(job_id): axum::extract::Path<String>,
) -> Result<Response, RegistryError> {
    let Some(job) = state.registry.get_gc_job(&job_id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let stats = job.stats.and_then(|s| serde_json::from_str(&s).ok());
    let resp = GcJobResponse {
        id: job.id,
        status: job.status,
        started_at: job.started_at,
        completed_at: job.completed_at,
        stats,
        created_at: job.created_at,
    };
    Ok(axum::Json(resp).into_response())
}
