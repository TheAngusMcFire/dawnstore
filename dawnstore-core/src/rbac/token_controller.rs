use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::{DateTime, Duration, Utc};
use dawnstore_lib::{IssueTokenRequest, IssueTokenResponse};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::authz_service::is_superadmin;
use super::constants::{API_VERSION_V1, KIND_SERVICE_ACCOUNT, KIND_SERVICE_ACCOUNT_TOKEN};
use super::helpers::object_string_id;
use super::jwt_service;
use super::middleware::Claims;
use super::models::ServiceAccountToken;
use crate::{
    abstractions::{DawnstoreBackend, Object},
    controllers,
};

// ── State ─────────────────────────────────────────────────────────────────────

pub(super) struct TokenState<B> {
    pub backend: Arc<B>,
    pub private_key_pem: Vec<u8>,
}

impl<B> Clone for TokenState<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            private_key_pem: self.private_key_pem.clone(),
        }
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

pub(super) fn routes<B: DawnstoreBackend + 'static>(state: TokenState<B>) -> Router {
    Router::new()
        .route("/rbac/issue-token", post(issue_token::<B>))
        .with_state(state)
}

// ── Handler ───────────────────────────────────────────────────────────────────

async fn issue_token<B: DawnstoreBackend + 'static>(
    State(state): State<TokenState<B>>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<IssueTokenRequest>,
) -> Response {
    if !is_superadmin(&claims) {
        return (StatusCode::FORBIDDEN, "only superadmin may issue tokens").into_response();
    }

    let expires_at = req
        .expires_at
        .unwrap_or_else(|| Utc::now() + Duration::days(365));

    let sa_ref = object_string_id(&req.namespace, KIND_SERVICE_ACCOUNT, &req.service_account);

    let token_obj: Object<ServiceAccountToken> = Object {
        api_version: Some(API_VERSION_V1.to_string()),
        kind: Some(KIND_SERVICE_ACCOUNT_TOKEN.to_string()),
        namespace: Some(req.namespace.clone()),
        name: req.token_name.clone(),
        spec: ServiceAccountToken {
            service_account: sa_ref,
            expires_at: Some(expires_at),
        },
        id: None,
        created_at: None,
        updated_at: None,
        annotations: None,
        labels: None,
    };

    let result = match state.backend.apply(token_obj).await {
        Ok(r) => r,
        Err(e) => return controllers::api_err(e),
    };

    let token_id = match result.first() {
        Some(o) => o.id,
        None => {
            return controllers::api_err(crate::error::DawnStoreError::InternalServerError(
                "apply returned not object".to_string(),
            ));
        }
    };

    let token = match jwt_service::create_token(
        &req.service_account,
        &req.namespace,
        &req.token_name,
        token_id,
        expires_at,
        &state.private_key_pem,
    ) {
        Ok(t) => t,
        Err(e) => return controllers::api_err(crate::error::DawnStoreError::JwtError(e)),
    };

    controllers::ok(IssueTokenResponse {
        token,
        token_id,
        expires_at,
    })
}
