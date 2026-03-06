use std::fmt;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    BlobUnknown,
    BlobUploadInvalid,
    BlobUploadUnknown,
    DigestInvalid,
    ManifestBlobUnknown,
    ManifestInvalid,
    ManifestUnknown,
    NameInvalid,
    NameUnknown,
    SizeInvalid,
    Unauthorized,
    Denied,
    Unsupported,
    TooManyRequests,
}

impl ErrorCode {
    #[must_use]
    pub const fn status_code(self) -> StatusCode {
        match self {
            Self::BlobUnknown
            | Self::BlobUploadUnknown
            | Self::ManifestBlobUnknown
            | Self::ManifestUnknown
            | Self::NameUnknown => StatusCode::NOT_FOUND,
            Self::DigestInvalid | Self::ManifestInvalid | Self::NameInvalid | Self::SizeInvalid => {
                StatusCode::BAD_REQUEST
            }
            Self::BlobUploadInvalid => StatusCode::RANGE_NOT_SATISFIABLE,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Denied => StatusCode::FORBIDDEN,
            Self::Unsupported => StatusCode::METHOD_NOT_ALLOWED,
            Self::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
        }
    }

    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::BlobUnknown => "blob unknown to registry",
            Self::BlobUploadInvalid => "blob upload invalid",
            Self::BlobUploadUnknown => "blob upload unknown to registry",
            Self::DigestInvalid => "provided digest did not match uploaded content",
            Self::ManifestBlobUnknown => {
                "manifest references a manifest or blob unknown to registry"
            }
            Self::ManifestInvalid => "manifest invalid",
            Self::ManifestUnknown => "manifest unknown to registry",
            Self::NameInvalid => "invalid repository name",
            Self::NameUnknown => "repository name not known to registry",
            Self::SizeInvalid => "provided length did not match content length",
            Self::Unauthorized => "authentication required",
            Self::Denied => "requested access to the resource is denied",
            Self::Unsupported => "the operation is unsupported",
            Self::TooManyRequests => "too many requests",
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::BlobUnknown => "BLOB_UNKNOWN",
            Self::BlobUploadInvalid => "BLOB_UPLOAD_INVALID",
            Self::BlobUploadUnknown => "BLOB_UPLOAD_UNKNOWN",
            Self::DigestInvalid => "DIGEST_INVALID",
            Self::ManifestBlobUnknown => "MANIFEST_BLOB_UNKNOWN",
            Self::ManifestInvalid => "MANIFEST_INVALID",
            Self::ManifestUnknown => "MANIFEST_UNKNOWN",
            Self::NameInvalid => "NAME_INVALID",
            Self::NameUnknown => "NAME_UNKNOWN",
            Self::SizeInvalid => "SIZE_INVALID",
            Self::Unauthorized => "UNAUTHORIZED",
            Self::Denied => "DENIED",
            Self::Unsupported => "UNSUPPORTED",
            Self::TooManyRequests => "TOOMANYREQUESTS",
        }
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct OciError {
    pub code: ErrorCode,
    pub message: String,
    pub detail: Option<serde_json::Value>,
}

impl OciError {
    #[must_use]
    pub fn new(code: ErrorCode) -> Self {
        Self {
            message: code.message().to_owned(),
            code,
            detail: None,
        }
    }

    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    #[must_use]
    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = Some(detail);
        self
    }

    #[must_use]
    pub fn blob_unknown() -> Self {
        Self::new(ErrorCode::BlobUnknown)
    }

    pub fn blob_upload_invalid(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::BlobUploadInvalid)
            .with_detail(serde_json::Value::String(detail.into()))
    }

    #[must_use]
    pub fn blob_upload_unknown() -> Self {
        Self::new(ErrorCode::BlobUploadUnknown)
    }

    pub fn digest_invalid(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::DigestInvalid).with_detail(serde_json::Value::String(detail.into()))
    }

    pub fn manifest_blob_unknown(digest: impl Into<String>) -> Self {
        Self::new(ErrorCode::ManifestBlobUnknown)
            .with_detail(serde_json::Value::String(digest.into()))
    }

    pub fn manifest_invalid(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::ManifestInvalid).with_detail(serde_json::Value::String(detail.into()))
    }

    pub fn manifest_unknown(reference: impl Into<String>) -> Self {
        Self::new(ErrorCode::ManifestUnknown)
            .with_detail(serde_json::Value::String(reference.into()))
    }

    pub fn name_invalid(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::NameInvalid).with_detail(serde_json::Value::String(detail.into()))
    }

    pub fn name_unknown(name: impl Into<String>) -> Self {
        Self::new(ErrorCode::NameUnknown).with_detail(serde_json::Value::String(name.into()))
    }

    pub fn size_invalid(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::SizeInvalid).with_detail(serde_json::Value::String(detail.into()))
    }

    #[must_use]
    pub fn unauthorized() -> Self {
        Self::new(ErrorCode::Unauthorized)
    }

    #[must_use]
    pub fn denied() -> Self {
        Self::new(ErrorCode::Denied)
    }

    #[must_use]
    pub fn unsupported() -> Self {
        Self::new(ErrorCode::Unsupported)
    }

    #[must_use]
    pub fn too_many_requests() -> Self {
        Self::new(ErrorCode::TooManyRequests)
    }
}

impl fmt::Display for OciError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for OciError {}

#[derive(Serialize)]
struct ErrorEntry<'a> {
    code: &'a ErrorCode,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: &'a Option<serde_json::Value>,
}

#[derive(Serialize)]
struct ErrorResponse<'a> {
    errors: Vec<ErrorEntry<'a>>,
}

impl IntoResponse for OciError {
    fn into_response(self) -> Response {
        let status = self.code.status_code();
        let body = ErrorResponse {
            errors: vec![ErrorEntry {
                code: &self.code,
                message: &self.message,
                detail: &self.detail,
            }],
        };
        (status, axum::Json(body)).into_response()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("{0}")]
    Oci(OciError),

    #[error("storage error: {0}")]
    Storage(#[from] anyhow::Error),

    #[error("database error: {0}")]
    Database(#[from] sea_orm::DbErr),
}

impl From<OciError> for RegistryError {
    fn from(err: OciError) -> Self {
        Self::Oci(err)
    }
}

impl IntoResponse for RegistryError {
    fn into_response(self) -> Response {
        match self {
            Self::Oci(err) => err.into_response(),
            Self::Storage(_) | Self::Database(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(ErrorResponse {
                    errors: vec![ErrorEntry {
                        code: &ErrorCode::BlobUnknown,
                        message: "internal server error",
                        detail: &None,
                    }],
                }),
            )
                .into_response(),
        }
    }
}
