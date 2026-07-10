use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::jwt_service::validate_token;
pub use super::jwt_service::Claims;
use crate::abstractions::{BackendGetObjectsFilter, DawnstoreBackend};
use crate::rbac::constants::KIND_SERVICE_ACCOUNT_TOKEN;

/// Axum middleware state holding the EC public key and the backend used to
/// verify that a token has not been revoked.
pub struct JwtAuthState<B> {
    pub public_key_pem: Vec<u8>,
    pub backend: Arc<B>,
}

// Manual `Clone` so the state is cloneable regardless of whether `B: Clone`
// (`Arc<B>` is always cloneable).
impl<B> Clone for JwtAuthState<B> {
    fn clone(&self) -> Self {
        Self {
            public_key_pem: self.public_key_pem.clone(),
            backend: Arc::clone(&self.backend),
        }
    }
}

/// Axum middleware that validates the `Authorization: Bearer <token>` header.
///
/// On success, inserts the validated [`Claims`] into request extensions so
/// downstream handlers can extract them via `Extension<Claims>`.
///
/// Revocation is checked against the backend on every request: the
/// `ServiceAccountToken` object identified by `claims.token_id` must still
/// exist. Deleting that object (directly, or via namespace deletion) revokes
/// every JWT derived from it immediately and consistently across all replicas —
/// there is no in-memory token cache to keep in sync.
///
/// Returns `401` if the header is missing, the token is invalid/expired, or the
/// corresponding `ServiceAccountToken` object has been deleted (revoked).
pub async fn jwt_auth_middleware<B: DawnstoreBackend + 'static>(
    State(state): State<JwtAuthState<B>>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = match extract_bearer(&request) {
        Some(t) => t,
        None => {
            return (StatusCode::UNAUTHORIZED, "missing Authorization header").into_response();
        }
    };

    let claims = match validate_token(token, &state.public_key_pem) {
        Ok(claims) => claims,
        Err(_e) => {
            // Generic message so token-validation internals are not exposed.
            return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
        }
    };

    // Revocation check against the backend (source of truth).
    let filter = BackendGetObjectsFilter {
        kind: Some(KIND_SERVICE_ACCOUNT_TOKEN.to_string()),
        ids: Some(vec![claims.token_id]),
        ..Default::default()
    };
    match state.backend.get_objects(&filter).await {
        Ok(objs) if !objs.is_empty() => {
            request.extensions_mut().insert(claims);
            next.run(request).await
        }
        Ok(_) => (StatusCode::UNAUTHORIZED, "token has been revoked").into_response(),
        Err(_e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "token validation failed").into_response()
        }
    }
}

fn extract_bearer(request: &Request) -> Option<&str> {
    request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}
