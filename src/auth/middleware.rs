use axum::extract::Request;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use jsonwebtoken::{DecodingKey, TokenData, Validation};
use serde::Deserialize;

use crate::api::AppState;
use crate::types::TenantId;

/// Authentication information extracted from the request.
#[derive(Clone, Debug)]
pub struct AuthInfo {
    pub tenant_id: TenantId,
    pub authenticated: bool,
}

impl AuthInfo {
    #[must_use]
    pub fn anonymous() -> Self {
        Self {
            tenant_id: TenantId::default_tenant(),
            authenticated: false,
        }
    }
}

/// JWT claims expected in Bearer tokens.
#[derive(Debug, Deserialize)]
struct Claims {
    /// Optional tenant identifier.
    #[serde(default)]
    tenant_id: Option<String>,
    /// Standard expiry — validated by `jsonwebtoken` during decode.
    #[allow(dead_code)]
    #[serde(default)]
    exp: Option<u64>,
}

/// Auth middleware. Extracts auth info from `Authorization` header.
/// When `auth.enabled` is true, rejects unauthenticated requests with
/// 401 and a `WWW-Authenticate` challenge header.
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let auth_info = extract_auth_info(&req, &state);

    if state.auth_config.enabled && !auth_info.authenticated {
        let is_read = req.method() == "GET" || req.method() == "HEAD";
        let is_data_plane = req.uri().path().starts_with("/v2/");
        if !(state.auth_config.anonymous_pull && is_read && is_data_plane) {
            return (
                StatusCode::UNAUTHORIZED,
                [(
                    "WWW-Authenticate",
                    "Bearer realm=\"/v2/token\",service=\"distribution-rs\"",
                )],
            )
                .into_response();
        }
    }

    req.extensions_mut().insert(auth_info);
    next.run(req).await
}

fn extract_auth_info(req: &Request, state: &AppState) -> AuthInfo {
    let Some(auth_header) = req.headers().get(AUTHORIZATION) else {
        return AuthInfo::anonymous();
    };
    let Ok(value) = auth_header.to_str() else {
        return AuthInfo::anonymous();
    };

    // Bearer token (JWT)
    if let Some(token) = value.strip_prefix("Bearer ") {
        if token.is_empty() {
            return AuthInfo::anonymous();
        }
        return validate_bearer(token, state);
    }

    // Basic auth
    if let Some(credentials) = value.strip_prefix("Basic ") {
        if credentials.is_empty() {
            return AuthInfo::anonymous();
        }
        return validate_basic(credentials, state);
    }

    AuthInfo::anonymous()
}

fn validate_bearer(token: &str, state: &AppState) -> AuthInfo {
    let Some(ref secret) = state.auth_config.jwt_secret else {
        // No JWT secret configured — accept any non-empty Bearer token
        // (stage 1 permissive mode).
        return AuthInfo {
            tenant_id: TenantId::default_tenant(),
            authenticated: true,
        };
    };

    let key = DecodingKey::from_secret(secret.as_bytes());
    let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);
    // Allow tokens without `exp` when in development.
    validation.required_spec_claims.clear();
    validation.validate_exp = true;

    let result: Result<TokenData<Claims>, _> = jsonwebtoken::decode(token, &key, &validation);

    match result {
        Ok(token_data) => {
            let tenant_id = token_data
                .claims
                .tenant_id
                .and_then(|t| t.parse::<TenantId>().ok())
                .unwrap_or_else(TenantId::default_tenant);
            AuthInfo {
                tenant_id,
                authenticated: true,
            }
        }
        Err(_) => AuthInfo::anonymous(),
    }
}

fn validate_basic(encoded: &str, state: &AppState) -> AuthInfo {
    let Ok(decoded_bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
        return AuthInfo::anonymous();
    };
    let Ok(decoded) = String::from_utf8(decoded_bytes) else {
        return AuthInfo::anonymous();
    };

    if state.auth_config.basic_credentials.is_empty() {
        // No credentials configured — accept any valid Basic auth
        // (stage 1 permissive mode).
        return AuthInfo {
            tenant_id: TenantId::default_tenant(),
            authenticated: true,
        };
    }

    // Check against configured credentials.
    if state
        .auth_config
        .basic_credentials
        .iter()
        .any(|c| c == &decoded)
    {
        return AuthInfo {
            tenant_id: TenantId::default_tenant(),
            authenticated: true,
        };
    }

    AuthInfo::anonymous()
}
