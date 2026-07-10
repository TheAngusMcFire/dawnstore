pub mod constants;
pub mod helpers;
pub mod jwt_service;
pub mod middleware;
pub mod models;

pub use constants::*;
pub use helpers::object_string_id;

use std::sync::Arc;

use axum::{Router, middleware::from_fn_with_state};
use chrono::{Duration, Utc};
use middleware::{JwtAuthState, jwt_auth_middleware};
use models::*;
use crate::abstractions::{BackendGetObjectsFilter, DawnstoreBackend, ForeignKey, ForeignKeyType, ObjectAny, SchemaDefinition};
use crate::error::DawnStoreError;

// ── Schema definitions ────────────────────────────────────────────────────────

pub fn schemas() -> Vec<SchemaDefinition> {
    vec![
        SchemaDefinition::new::<Namespace>(API_VERSION_V1, KIND_NAMESPACE, ["namespaces", "ns"], []),
        SchemaDefinition::new::<ServiceAccount>(
            API_VERSION_V1, KIND_SERVICE_ACCOUNT, ["serviceaccounts", "sa"], [],
        ),
        SchemaDefinition::new::<ServiceAccountToken>(
            API_VERSION_V1,
            KIND_SERVICE_ACCOUNT_TOKEN,
            ["serviceaccounttokens", "sat"],
            [ForeignKey::new(
                "service_account",
                None::<&str>,
                ForeignKeyType::One,
                Some(KIND_SERVICE_ACCOUNT),
            )],
        ),
        SchemaDefinition::new::<Role>(API_VERSION_V1, KIND_ROLE, ["roles", "ro"], []),
        SchemaDefinition::new::<GlobalRole>(API_VERSION_V1, KIND_GLOBAL_ROLE, ["globalroles", "gr"], []),
        SchemaDefinition::new::<RoleBinding>(
            API_VERSION_V1,
            KIND_ROLE_BINDING,
            ["rolebindings", "rb"],
            [
                ForeignKey::new("role", None::<&str>, ForeignKeyType::One, Some(KIND_ROLE)),
                ForeignKey::new(
                    "subjects",
                    None::<&str>,
                    ForeignKeyType::OneOrMany,
                    Some(KIND_SERVICE_ACCOUNT),
                ),
            ],
        ),
        SchemaDefinition::new::<GlobalRoleBinding>(
            API_VERSION_V1,
            KIND_GLOBAL_ROLE_BINDING,
            ["globalrolebindings", "grb"],
            [
                ForeignKey::new("role", None::<&str>, ForeignKeyType::One, Some(KIND_GLOBAL_ROLE)),
                ForeignKey::new(
                    "subjects",
                    None::<&str>,
                    ForeignKeyType::OneOrMany,
                    Some(KIND_SERVICE_ACCOUNT),
                ),
            ],
        ),
    ]
}

// ── Seeding ───────────────────────────────────────────────────────────────────

async fn seed_namespace<B: DawnstoreBackend>(backend: &B, name: &str) -> Result<(), DawnStoreError> {
    let obj = crate::abstractions::Object {
        api_version: Some(API_VERSION_V1.to_string()),
        kind: Some(KIND_NAMESPACE.to_string()),
        namespace: Some(SYSTEM_NAMESPACE.to_string()),
        name: name.to_string(),
        spec: Namespace {},
        id: None,
        created_at: None,
        updated_at: None,
        annotations: None,
        labels: None,
    };
    let obj_any: ObjectAny = serde_json::from_value(serde_json::to_value(obj)?)?;
    backend.upsert_objects(vec![obj_any], vec![]).await?;
    Ok(())
}

async fn seed_superadmin<B: DawnstoreBackend>(backend: &B) -> Result<(), DawnStoreError> {
    let obj = crate::abstractions::Object {
        api_version: Some(API_VERSION_V1.to_string()),
        kind: Some(KIND_SERVICE_ACCOUNT.to_string()),
        namespace: Some(SYSTEM_NAMESPACE.to_string()),
        name: SA_SUPERADMIN.to_string(),
        spec: ServiceAccount {},
        id: None,
        created_at: None,
        updated_at: None,
        annotations: None,
        labels: None,
    };
    let obj_any: ObjectAny = serde_json::from_value(serde_json::to_value(obj)?)?;
    backend.upsert_objects(vec![obj_any], vec![]).await?;
    Ok(())
}

/// Seed all RBAC schemas, the `system` and `default` namespaces, and the
/// `superadmin` service account.
///
/// Idempotent — safe to call on every startup.
pub async fn init<B: DawnstoreBackend>(backend: &B) -> Result<(), DawnStoreError> {
    backend.seed_schemas(&schemas()).await?;
    seed_namespace(backend, SYSTEM_NAMESPACE).await?;
    seed_namespace(backend, "default").await?;
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
        .get_objects(&BackendGetObjectsFilter {
            namespace: Some(SYSTEM_NAMESPACE.to_string()),
            kind: Some(KIND_SERVICE_ACCOUNT_TOKEN.to_string()),
            name: Some(TOKEN_BOOTSTRAP.to_string()),
            ..Default::default()
        })
        .await?;

    if !existing.is_empty() {
        return Ok(None);
    }

    let expires_at = Utc::now() + Duration::days(365);

    let obj = crate::abstractions::Object {
        api_version: Some(API_VERSION_V1.to_string()),
        kind: Some(KIND_SERVICE_ACCOUNT_TOKEN.to_string()),
        namespace: Some(SYSTEM_NAMESPACE.to_string()),
        name: TOKEN_BOOTSTRAP.to_string(),
        spec: ServiceAccountToken {
            service_account: object_string_id(SYSTEM_NAMESPACE, KIND_SERVICE_ACCOUNT, SA_SUPERADMIN),
            expires_at: Some(expires_at),
            service_account_object: None,
        },
        id: None,
        created_at: None,
        updated_at: None,
        annotations: None,
        labels: None,
    };
    let obj_any: ObjectAny = serde_json::from_value(serde_json::to_value(obj)?)?;
    let result = backend.upsert_objects(vec![obj_any], vec![]).await?;

    let token_id = result
        .first()
        .ok_or_else(|| DawnStoreError::InternalServerError("apply returned no object".into()))?
        .id;

    let token = jwt_service::create_token(
        SA_SUPERADMIN,
        SYSTEM_NAMESPACE,
        TOKEN_BOOTSTRAP,
        token_id,
        expires_at,
        private_key_pem,
    )
    .map_err(|e| DawnStoreError::InternalServerError(e.to_string()))?;

    Ok(Some(token))
}

// ── Axum router helpers ───────────────────────────────────────────────────────

/// Wrap `router` with JWT authentication middleware.
///
/// `public_key_pem` is the PEM-encoded EC public key used to verify tokens.
/// `backend` is queried on every request to check whether the token's backing
/// `ServiceAccountToken` object still exists (revocation is DB-backed, not cached).
pub fn with_jwt_auth<B: DawnstoreBackend + 'static>(
    router: Router,
    public_key_pem: Vec<u8>,
    backend: Arc<B>,
) -> Router {
    let state = JwtAuthState { public_key_pem, backend };
    router.layer(from_fn_with_state(state, jwt_auth_middleware::<B>))
}
