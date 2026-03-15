use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::jwt_service::validate_token;
pub use super::jwt_service::Claims;
use crate::cache::DawnstoreCache;

/// Axum middleware state holding the EC public key and the token-revocation cache.
#[derive(Clone)]
pub struct JwtAuthState {
    pub public_key_pem: Vec<u8>,
    pub cache: Arc<DawnstoreCache>,
}

/// Axum middleware that validates the `Authorization: Bearer <token>` header.
///
/// On success, inserts the validated [`Claims`] into request extensions so
/// downstream handlers can extract them via `Extension<Claims>`.
/// Returns `401` if the header is missing, the token is invalid, or the
/// corresponding `ServiceAccountToken` object has been deleted (revoked).
pub async fn jwt_auth_middleware(
    State(state): State<JwtAuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = match extract_bearer(&request) {
        Some(t) => t,
        None => {
            return (StatusCode::UNAUTHORIZED, "missing Authorization header").into_response();
        }
    };

    match validate_token(token, &state.public_key_pem) {
        Ok(claims) => {
            if !state.cache.is_token_valid(claims.token_id) {
                return (StatusCode::UNAUTHORIZED, "token has been revoked").into_response();
            }
            request.extensions_mut().insert(claims);
            next.run(request).await
        }
        Err(e) => (StatusCode::UNAUTHORIZED, e.to_string()).into_response(),
    }
}

fn extract_bearer(request: &Request) -> Option<&str> {
    request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}
