pub mod authz_service;
pub mod jwt_service;
pub mod middleware;
pub mod models;
mod token_controller;

use std::sync::Arc;

use axum::{Router, middleware::from_fn_with_state};
use chrono::{Duration, Utc};
use middleware::{JwtAuthState, jwt_auth_middleware};
use models::*;
use crate::abstractions::{DawnstoreBackend, ForeignKey, ForeignKeyType, GetObjectsFilter, Object, SchemaDefinition};
use crate::error::DawnStoreError;

// ── Schema definitions ────────────────────────────────────────────────────────

pub fn schemas() -> Vec<SchemaDefinition> {
    vec![
        SchemaDefinition::new::<Namespace>("v1", "namespace", ["namespaces", "ns"], []),
        SchemaDefinition::new::<ServiceAccount>(
            "v1", "serviceaccount", ["serviceaccounts", "sa"], [],
        ),
        SchemaDefinition::new::<ServiceAccountToken>(
            "v1",
            "serviceaccounttoken",
            ["serviceaccounttokens", "sat"],
            [ForeignKey::new(
                "service_account",
                None::<&str>,
                ForeignKeyType::One,
                Some("serviceaccount"),
            )],
        ),
        SchemaDefinition::new::<Role>("v1", "role", ["roles", "ro"], []),
        SchemaDefinition::new::<GlobalRole>("v1", "globalrole", ["globalroles", "gr"], []),
        SchemaDefinition::new::<RoleBinding>(
            "v1",
            "rolebinding",
            ["rolebindings", "rb"],
            [
                ForeignKey::new("role", None::<&str>, ForeignKeyType::One, Some("role")),
                ForeignKey::new(
                    "subjects",
                    None::<&str>,
                    ForeignKeyType::OneOrMany,
                    Some("serviceaccount"),
                ),
            ],
        ),
        SchemaDefinition::new::<GlobalRoleBinding>(
            "v1",
            "globalrolebinding",
            ["globalrolebindings", "grb"],
            [
                ForeignKey::new("role", None::<&str>, ForeignKeyType::One, Some("globalrole")),
                ForeignKey::new(
                    "subjects",
                    None::<&str>,
                    ForeignKeyType::OneOrMany,
                    Some("serviceaccount"),
                ),
            ],
        ),
    ]
}

// ── Seeding ───────────────────────────────────────────────────────────────────

async fn seed_system_namespace<B: DawnstoreBackend>(backend: &B) -> Result<(), DawnStoreError> {
    backend
        .apply(Object {
            api_version: Some("v1".to_string()),
            kind: Some("namespace".to_string()),
            namespace: Some("system".to_string()),
            name: "system".to_string(),
            spec: Namespace {},
            id: None,
            created_at: None,
            updated_at: None,
            annotations: None,
            labels: None,
        })
        .await?;
    Ok(())
}

async fn seed_superadmin<B: DawnstoreBackend>(backend: &B) -> Result<(), DawnStoreError> {
    backend
        .apply(Object {
            api_version: Some("v1".to_string()),
            kind: Some("serviceaccount".to_string()),
            namespace: Some("system".to_string()),
            name: "superadmin".to_string(),
            spec: ServiceAccount {},
            id: None,
            created_at: None,
            updated_at: None,
            annotations: None,
            labels: None,
        })
        .await?;
    Ok(())
}

/// Seed all RBAC schemas, the `system` namespace, and the `superadmin` service account.
///
/// Idempotent — safe to call on every startup.
pub async fn init<B: DawnstoreBackend>(backend: &B) -> Result<(), DawnStoreError> {
    backend.seed_schema(&schemas()).await?;
    seed_system_namespace(backend).await?;
    seed_superadmin(backend).await
}

/// Bootstrap a fresh instance.
///
/// On the **first** startup (detected by the absence of a `system/serviceaccounttoken/bootstrap`
/// object) this function creates that token and returns the signed JWT so the caller can print
/// it to stdout. On subsequent startups it returns `None`.
///
/// The returned token is valid for 1 year. After bootstrapping, rotate it using the normal
/// `POST /rbac/issue-token` endpoint.
pub async fn bootstrap<B: DawnstoreBackend>(
    backend: &B,
    private_key_pem: &[u8],
) -> Result<Option<String>, DawnStoreError> {
    let existing = backend
        .get(&GetObjectsFilter {
            namespace: Some("system".to_string()),
            kind: Some("serviceaccounttoken".to_string()),
            name: Some("bootstrap".to_string()),
            ..Default::default()
        })
        .await?;

    if !existing.is_empty() {
        return Ok(None);
    }

    let expires_at = Utc::now() + Duration::days(365);

    let result = backend
        .apply(Object {
            api_version: Some("v1".to_string()),
            kind: Some("serviceaccounttoken".to_string()),
            namespace: Some("system".to_string()),
            name: "bootstrap".to_string(),
            spec: ServiceAccountToken {
                service_account: "system/serviceaccount/superadmin".to_string(),
                expires_at: Some(expires_at),
            },
            id: None,
            created_at: None,
            updated_at: None,
            annotations: None,
            labels: None,
        })
        .await?;

    let token_id = result
        .first()
        .ok_or_else(|| DawnStoreError::InternalServerError("apply returned no object".into()))?
        .id;

    let token = jwt_service::create_token(
        "superadmin",
        "system",
        "bootstrap",
        token_id,
        expires_at,
        private_key_pem,
    )
    .map_err(|e| DawnStoreError::InternalServerError(e.to_string()))?;

    Ok(Some(token))
}

// ── Axum router helpers ───────────────────────────────────────────────────────

/// Returns the RBAC management routes (e.g. `POST /rbac/issue-token`).
///
/// These routes must be merged into the application router **before** calling
/// [`with_jwt_auth`], so that the JWT middleware protects them.
pub fn get_rbac_routes<B: DawnstoreBackend + 'static>(
    backend: Arc<B>,
    private_key_pem: Vec<u8>,
) -> Router {
    token_controller::routes(token_controller::TokenState { backend, private_key_pem })
}

/// Wrap `router` with JWT authentication middleware.
///
/// `public_key_pem` is the PEM-encoded EC public key used to verify tokens.
pub fn with_jwt_auth(router: Router, public_key_pem: Vec<u8>) -> Router {
    let state = JwtAuthState { public_key_pem };
    router.layer(from_fn_with_state(state, jwt_auth_middleware))
}
