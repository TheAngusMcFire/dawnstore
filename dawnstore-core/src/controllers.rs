use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, post},
};
use chrono::{Duration, Utc};
use dawnstore_lib::*;

use crate::abstractions::{DawnstoreBackend, ObjectAny, Object};
use crate::cache::DawnstoreCache;
use crate::error::DawnStoreError;
use crate::rbac::constants::{
    API_VERSION_V1, KIND_NAMESPACE, KIND_SERVICE_ACCOUNT, KIND_SERVICE_ACCOUNT_TOKEN, SYSTEM_NAMESPACE,
};
use crate::rbac::models::ServiceAccountToken;
use crate::cache::is_superadmin;
use crate::rbac::jwt_service;
use crate::rbac::helpers::object_string_id;
use crate::rbac::middleware::Claims;
use crate::{handlers::apply, handlers::delete as delete_handler, handlers::get as get_handler};

// ── Error mapping ─────────────────────────────────────────────────────────────

/// Map an internal [`DawnStoreError`] to a client-safe [`DawnStoreApiError`].
/// Database internals, stack traces, and other sensitive details are stripped.
fn to_api_error(err: DawnStoreError) -> DawnStoreApiError {
    match err {
        DawnStoreError::UnknownResourceKind(kind) => {
            DawnStoreApiError::UnknownResourceKind { kind }
        }
        DawnStoreError::NamespaceCanOnlyBeCreatedInSystemNamespace(namespace) => {
            DawnStoreApiError::NamespaceRestriction { namespace }
        }
        DawnStoreError::NoSchemaForObjectFound { api_version, kind } => {
            DawnStoreApiError::SchemaNotFound { api_version, kind }
        }
        DawnStoreError::ObjectValidationError {
            name,
            validation_error,
            ..
        } => DawnStoreApiError::ValidationError {
            name,
            message: validation_error.to_string(),
        },
        DawnStoreError::ObjectValidationMissingForeignKeyEntry {
            name,
            foreign_key_path,
            ..
        } => DawnStoreApiError::ValidationError {
            name,
            message: format!("missing required foreign key field: {foreign_key_path}"),
        },
        DawnStoreError::ObjectValidationWrongForeignKeyEntryFormat {
            name,
            foreign_key_path,
            value,
            ..
        } => DawnStoreApiError::ValidationError {
            name,
            message: format!("invalid foreign key format at '{foreign_key_path}': {value}"),
        },
        DawnStoreError::ObjectValidationWrongForeignKeyEntryKind {
            name,
            foreign_key_path,
            value,
            ..
        } => DawnStoreApiError::ValidationError {
            name,
            message: format!("wrong foreign key kind at '{foreign_key_path}': {value}"),
        },
        DawnStoreError::ObjectValidationForeignKeyNotFound { value, .. } => {
            DawnStoreApiError::ForeignKeyNotFound { value }
        }
        DawnStoreError::ForeignKeyNotFound(value) => {
            DawnStoreApiError::ForeignKeyNotFound { value }
        }
        DawnStoreError::InvalidObjectName(name) => DawnStoreApiError::ValidationError {
            name,
            message: "object name must not contain '/'".to_string(),
        },
        DawnStoreError::InvalidObjectNamespace(ns) => DawnStoreApiError::InvalidInput {
            message: format!("object namespace '{ns}' must not contain '/'"),
        },
        DawnStoreError::InvalidRootInputObject
        | DawnStoreError::InvalidInputObjectMissingKindField
        | DawnStoreError::InvalidInputObjectMissingListFieldOfList
        | DawnStoreError::KindMissingInObject
        | DawnStoreError::ApiVersionMissingInObject => DawnStoreApiError::InvalidInput {
            message: err.to_string(),
        },
        DawnStoreError::DeserialisationError(e) => DawnStoreApiError::InvalidInput {
            message: e.to_string(),
        },
        DawnStoreError::Forbidden => DawnStoreApiError::Forbidden,
        DawnStoreError::DeleteBlockedByReferences { target, referencing } => {
            DawnStoreApiError::ValidationError {
                name: target,
                message: format!("object is still referenced by: {referencing}"),
            }
        }
        DawnStoreError::NamespaceNotFound(ns) => DawnStoreApiError::ValidationError {
            name: ns.clone(),
            message: format!("namespace '{ns}' does not exist"),
        },
        DawnStoreError::DeleteNamespaceBlockedByCrossNamespaceReferences {
            namespace,
            referencing,
        } => DawnStoreApiError::ValidationError {
            name: namespace,
            message: format!(
                "namespace cannot be deleted: objects inside it are still referenced from other namespaces by: {referencing}"
            ),
        },
        DawnStoreError::DatabaseError(_)
        | DawnStoreError::InternalServerError(_)
        | DawnStoreError::JsonSchemaValidatorCreationError(_) => DawnStoreApiError::InternalError,
        DawnStoreError::JwtError(_jwt_error) => DawnStoreApiError::InternalError,
    }
}

// ── Response helpers ──────────────────────────────────────────────────────────

pub fn ok<T: serde::Serialize>(data: T) -> Response {
    Json(DawnStoreResponse::ok(data)).into_response()
}

pub fn api_err(err: DawnStoreError) -> Response {
    Json(DawnStoreResponse::<()>::err(to_api_error(err))).into_response()
}

/// Return 401 for auth failures (not wrapped in the envelope).
fn auth_err(status: StatusCode, message: impl Into<String>) -> Response {
    let mut resp = message.into().into_response();
    *resp.status_mut() = status;
    resp
}

// ── Controller state ───────────────────────────────────────────────────────

struct ApiState<B> {
    backend: Arc<B>,
    cache: Arc<DawnstoreCache>,
    private_key_pem: Vec<u8>,
}

impl<B> Clone for ApiState<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            cache: Arc::clone(&self.cache),
            private_key_pem: self.private_key_pem.clone(),
        }
    }
}

pub fn get_dawnstore_routes<B>(
    backend: Arc<B>,
    cache: Arc<DawnstoreCache>,
    private_key_pem: Vec<u8>,
) -> Router
where
    B: DawnstoreBackend + 'static,
{
    Router::new()
        .route("/apply", post(apply_handler::<B>))
        .route("/get-objects", post(get_objects_handler::<B>))
        .route("/get-resource-definitions", post(get_resource_definitions_handler::<B>))
        .route("/delete-object", delete(delete_object_handler::<B>))
        .route("/rbac/issue-token", post(issue_token::<B>))
        .with_state(ApiState { backend, cache, private_key_pem })
}

/// Check that no `Namespace` objects are being applied outside the system namespace.
async fn check_namespace_restriction_cached(
    cache: &DawnstoreCache,
    value: &serde_json::Value,
) -> Result<(), DawnStoreError> {
    let objects: Vec<&serde_json::Value> = if let Some(arr) = value.as_array() {
        arr.iter().collect()
    } else if value.get("kind").and_then(|k| k.as_str()) == Some("list") {
        value
            .get("list")
            .and_then(|l| l.as_array())
            .map(|a| a.iter().collect())
            .unwrap_or_default()
    } else {
        vec![value]
    };

    for obj in objects {
        let kind = obj.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        if cache.resolve_kind(kind).await.as_deref() == Some(KIND_NAMESPACE) {
            let ns = obj.get("namespace").and_then(|n| n.as_str()).unwrap_or("default");
            if ns != SYSTEM_NAMESPACE {
                return Err(DawnStoreError::NamespaceCanOnlyBeCreatedInSystemNamespace(
                    ns.to_string(),
                ));
            }
        }
    }
    Ok(())
}

async fn apply_handler<B>(
    State(state): State<ApiState<B>>,
    claims_ext: Option<Extension<Claims>>,
    Json(obj): Json<serde_json::Value>,
) -> Response
where
    B: DawnstoreBackend + 'static,
{
    if let Err(e) = check_namespace_restriction_cached(&*state.cache, &obj).await {
        return api_err(e);
    }
    let caller = claims_ext.map(|e| e.0);
    match apply::apply(&*state.backend, &*state.cache, caller.as_ref(), obj).await {
        Ok(applied) => ok(applied),
        Err(e) => api_err(e),
    }
}

async fn get_objects_handler<B>(
    State(state): State<ApiState<B>>,
    claims_ext: Option<Extension<Claims>>,
    Json(mut filter): Json<GetObjectsFilter>,
) -> Response
where
    B: DawnstoreBackend + 'static,
{
    // Translate namespace=default (or absent) to system for Namespace kind queries.
    if let Some(k) = &filter.kind.clone() {
        if let Some(resolved) = state.cache.resolve_kind(k).await {
            if resolved == KIND_NAMESPACE
                && matches!(filter.namespace.as_deref(), None | Some("default"))
            {
                filter.namespace = Some(SYSTEM_NAMESPACE.to_string());
            }
        }
    }
    let caller = claims_ext.map(|e| e.0);
    match get_handler::get(&*state.backend, &*state.cache, caller.as_ref(), filter).await {
        Ok(x) => ok(x),
        Err(e) => api_err(e),
    }
}

async fn delete_object_handler<B>(
    State(state): State<ApiState<B>>,
    claims_ext: Option<Extension<Claims>>,
    Json(query): Json<DeleteObject>,
) -> Response
where
    B: DawnstoreBackend + 'static,
{
    let caller = claims_ext.map(|e| e.0);
    match delete_handler::delete(&*state.backend, &*state.cache, caller.as_ref(), query).await {
        Ok(()) => ok(true),
        Err(e) => api_err(e),
    }
}

async fn get_resource_definitions_handler<B>(
    State(state): State<ApiState<B>>,
    Json(_query): Json<GetResourceDefinitionFilter>,
) -> Response
where
    B: DawnstoreBackend + 'static,
{
    match state.backend.get_resource_definitions().await {
        Ok(x) => ok(x),
        Err(e) => api_err(e),
    }
}

// Keep the auth_err helper available for the JWT middleware to call if needed.
pub fn unauthorized(message: impl Into<String>) -> Response {
    auth_err(StatusCode::UNAUTHORIZED, message)
}

// ── Token issuance ────────────────────────────────────────────────────────────

async fn issue_token<B: DawnstoreBackend + 'static>(
    State(state): State<ApiState<B>>,
    Extension(claims): Extension<crate::rbac::middleware::Claims>,
    Json(req): Json<IssueTokenRequest>,
) -> Response {
    if !is_superadmin(&claims) {
        return (StatusCode::FORBIDDEN, "only superadmin may issue tokens").into_response();
    }

    // Reject names/namespaces containing '/' — these bypass the normal apply
    // path and would create ambiguous string IDs.
    if req.token_name.contains('/') {
        return api_err(DawnStoreError::InvalidObjectName(req.token_name.clone()));
    }
    if req.namespace.contains('/') {
        return api_err(DawnStoreError::InvalidObjectNamespace(req.namespace.clone()));
    }
    if req.service_account.contains('/') {
        return api_err(DawnStoreError::InvalidObjectName(req.service_account.clone()));
    }

    let expires_at = req
        .expires_at
        .unwrap_or_else(|| Utc::now() + Duration::days(365));

    let sa_ref = object_string_id(&req.namespace, KIND_SERVICE_ACCOUNT, &req.service_account);

    // Verify the ServiceAccount exists before creating a token for it.
    // The normal apply flow does this via FK graph walk; here we do it explicitly
    // since issue_token calls upsert_objects directly.
    match state.backend.get_object(&req.namespace, KIND_SERVICE_ACCOUNT, &req.service_account).await {
        Err(e) => return api_err(e),
        Ok(None) => return api_err(DawnStoreError::ObjectValidationForeignKeyNotFound {
            api_version: API_VERSION_V1.to_string(),
            kind: KIND_SERVICE_ACCOUNT_TOKEN.to_string(),
            name: req.token_name.clone(),
            value: sa_ref.clone(),
        }),
        Ok(Some(_)) => {} // SA exists — proceed
    }

    let token_obj: Object<ServiceAccountToken> = Object {
        api_version: Some(API_VERSION_V1.to_string()),
        kind: Some(KIND_SERVICE_ACCOUNT_TOKEN.to_string()),
        namespace: Some(req.namespace.clone()),
        name: req.token_name.clone(),
        spec: ServiceAccountToken {
            service_account: sa_ref,
            expires_at: Some(expires_at),
            service_account_object: None,
        },
        id: None,
        created_at: None,
        updated_at: None,
        annotations: None,
        labels: None,
    };

    let obj_any: ObjectAny = match serde_json::from_value(serde_json::to_value(token_obj).unwrap()) {
        Ok(v) => v,
        Err(e) => return api_err(DawnStoreError::DeserialisationError(e)),
    };

    let result = match state.backend.upsert_objects(vec![obj_any], vec![]).await {
        Ok(r) => r,
        Err(e) => return api_err(e),
    };

    let token_id = match result.first() {
        Some(o) => o.id,
        None => {
            return api_err(crate::error::DawnStoreError::InternalServerError(
                "apply returned no object".to_string(),
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
        Err(e) => return api_err(crate::error::DawnStoreError::JwtError(e)),
    };

    // Register the new token so it is immediately usable without waiting for a
    // cache rebuild, and so it can be revoked by deleting the object.
    state.cache.add_token(token_id);

    ok(IssueTokenResponse { token, token_id, expires_at })
}
