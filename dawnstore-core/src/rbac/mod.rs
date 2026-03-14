pub mod jwt_service;
pub mod middleware;
pub mod models;

use axum::{Router, middleware::from_fn_with_state};
use middleware::{JwtAuthState, jwt_auth_middleware};
use models::*;
use crate::abstractions::{DawnstoreBackend, ForeignKey, ForeignKeyType, Object, SchemaDefinition};
use crate::error::DawnStoreError;

pub fn schemas() -> Vec<SchemaDefinition> {
    vec![
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
                ForeignKey::new("subjects", None::<&str>, ForeignKeyType::OneOrMany, Some("serviceaccount")),
            ],
        ),
        SchemaDefinition::new::<GlobalRoleBinding>(
            "v1",
            "globalrolebinding",
            ["globalrolebindings", "grb"],
            [
                ForeignKey::new("role", None::<&str>, ForeignKeyType::One, Some("globalrole")),
                ForeignKey::new("subjects", None::<&str>, ForeignKeyType::OneOrMany, Some("serviceaccount")),
            ],
        ),
    ]
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

pub async fn init<B: DawnstoreBackend>(backend: &B) -> Result<(), DawnStoreError> {
    backend.seed_schema(&schemas()).await?;
    seed_superadmin(backend).await
}

/// Wrap `router` with JWT authentication middleware.
///
/// `public_key_pem` is the PEM-encoded EC public key used to verify tokens.
pub fn with_jwt_auth(router: Router, public_key_pem: Vec<u8>) -> Router {
    let state = JwtAuthState { public_key_pem };
    router.layer(from_fn_with_state(state, jwt_auth_middleware))
}
