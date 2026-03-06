use axum::http::StatusCode;
use axum::response::IntoResponse;

pub async fn version_check() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("Docker-Distribution-API-Version", "registry/2.0")],
    )
}
