use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, post},
};
use dawnstore_lib::*;

use crate::abstractions::DawnstoreBackend;
use crate::error::DawnStoreError;

pub fn get_dawnstore_default_routes<B>(backend: Arc<B>) -> Router
where
    B: DawnstoreBackend + 'static,
{
    Router::new()
        .route("/apply", post(apply::<B>))
        .route("/get-objects", post(get_objects::<B>))
        .route("/get-object-infos", post(get_object_infos::<B>))
        .route("/get-resource-definitions", post(get_resource_definitions::<B>))
        .route("/delete-object", delete(delete_object::<B>))
        .with_state(ApiState { backend })
}

struct ApiState<B> {
    backend: Arc<B>,
}

impl<B> Clone for ApiState<B> {
    fn clone(&self) -> Self {
        Self { backend: Arc::clone(&self.backend) }
    }
}

// ── Error mapping ─────────────────────────────────────────────────────────────

/// Map an internal [`DawnStoreError`] to a client-safe [`DawnStoreApiError`].
/// Database internals, stack traces, and other sensitive details are stripped.
fn to_api_error(err: DawnStoreError) -> DawnStoreApiError {
    match err {
        DawnStoreError::UnknownResourceKind(kind) => DawnStoreApiError::UnknownResourceKind { kind },
        DawnStoreError::NamespaceCanOnlyBeCreatedInSystemNamespace(namespace) => {
            DawnStoreApiError::NamespaceRestriction { namespace }
        }
        DawnStoreError::NoSchemaForObjectFound { api_version, kind } => {
            DawnStoreApiError::SchemaNotFound { api_version, kind }
        }
        DawnStoreError::ObjectValidationError { name, validation_error, .. } => {
            DawnStoreApiError::ValidationError { name, message: validation_error.to_string() }
        }
        DawnStoreError::ObjectValidationMissingForeignKeyEntry { name, foreign_key_path, .. } => {
            DawnStoreApiError::ValidationError {
                name,
                message: format!("missing required foreign key field: {foreign_key_path}"),
            }
        }
        DawnStoreError::ObjectValidationWrongForeignKeyEntryFormat { name, foreign_key_path, value, .. } => {
            DawnStoreApiError::ValidationError {
                name,
                message: format!("invalid foreign key format at '{foreign_key_path}': {value}"),
            }
        }
        DawnStoreError::ObjectValidationWrongForeignKeyEntryKind { name, foreign_key_path, value, .. } => {
            DawnStoreApiError::ValidationError {
                name,
                message: format!("wrong foreign key kind at '{foreign_key_path}': {value}"),
            }
        }
        DawnStoreError::ObjectValidationForeignKeyNotFound { value, .. } => {
            DawnStoreApiError::ForeignKeyNotFound { value }
        }
        DawnStoreError::ForeignKeyNotFound(value) => DawnStoreApiError::ForeignKeyNotFound { value },
        DawnStoreError::InvalidRootInputObject
        | DawnStoreError::InvalidInputObjectMissingKindField
        | DawnStoreError::InvalidInputObjectMissingListFieldOfList
        | DawnStoreError::KindMissingInObject
        | DawnStoreError::ApiVersionMissingInObject => {
            DawnStoreApiError::InvalidInput { message: err.to_string() }
        }
        DawnStoreError::DeserialisationError(e) => {
            DawnStoreApiError::InvalidInput { message: e.to_string() }
        }
        // Internal errors: do not leak details to the client.
        DawnStoreError::DatabaseError(_)
        | DawnStoreError::InternalServerError(_)
        | DawnStoreError::JsonSchemaValidatorCreationError(_) => DawnStoreApiError::InternalError,
    }
}

// ── Controller pre-checks ─────────────────────────────────────────────────────

/// Walk the raw apply payload (single object, array, or List wrapper) and
/// verify that no `namespace` object is being applied outside `system`.
fn check_namespace_restriction<B: DawnstoreBackend>(
    backend: &B,
    value: &serde_json::Value,
) -> Result<(), DawnStoreError> {
    let objects: Vec<&serde_json::Value> =
        if let Some(arr) = value.as_array() {
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
        if backend.resolve_kind(kind) == "namespace" {
            let ns = obj.get("namespace").and_then(|n| n.as_str()).unwrap_or("default");
            if ns != "system" {
                return Err(DawnStoreError::NamespaceCanOnlyBeCreatedInSystemNamespace(
                    ns.to_string(),
                ));
            }
        }
    }
    Ok(())
}

// ── Response helpers ──────────────────────────────────────────────────────────

fn ok<T: serde::Serialize>(data: T) -> Response {
    Json(DawnStoreResponse::ok(data)).into_response()
}

fn api_err(err: DawnStoreError) -> Response {
    Json(DawnStoreResponse::<()>::err(to_api_error(err))).into_response()
}

/// Return 401 for auth failures (not wrapped in the envelope).
fn auth_err(status: StatusCode, message: impl Into<String>) -> Response {
    let mut resp = message.into().into_response();
    *resp.status_mut() = status;
    resp
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn apply<B>(
    State(state): State<ApiState<B>>,
    Json(obj): Json<serde_json::Value>,
) -> Response
where
    B: DawnstoreBackend + 'static,
{
    if let Err(e) = check_namespace_restriction(&*state.backend, &obj) {
        return api_err(e);
    }
    match state.backend.apply_raw(obj).await {
        Ok(x) => ok(x),
        Err(e) => api_err(e),
    }
}

async fn get_objects<B>(
    State(state): State<ApiState<B>>,
    Json(mut query): Json<GetObjectsFilter>,
) -> Response
where
    B: DawnstoreBackend + 'static,
{
    if let Some(k) = &query.kind.clone() {
        let resolved = match state.backend.resource_cache().resolve(k) {
            Some(r) => r,
            None => return api_err(DawnStoreError::UnknownResourceKind(k.clone())),
        };
        if resolved == "namespace"
            && matches!(query.namespace.as_deref(), None | Some("default"))
        {
            query.namespace = Some("system".to_string());
        }
    }
    match state.backend.get(&query).await {
        Ok(x) => ok(x),
        Err(e) => api_err(e),
    }
}

async fn get_object_infos<B>(
    State(state): State<ApiState<B>>,
    Json(mut query): Json<GetObjectInfosFilter>,
) -> Response
where
    B: DawnstoreBackend + 'static,
{
    if let Some(k) = &query.kind.clone() {
        let resolved = match state.backend.resource_cache().resolve(k) {
            Some(r) => r,
            None => return api_err(DawnStoreError::UnknownResourceKind(k.clone())),
        };
        if resolved == "namespace"
            && matches!(query.namespace.as_deref(), None | Some("default"))
        {
            query.namespace = Some("system".to_string());
        }
    }
    match state.backend.get_object_infos(&query).await {
        Ok(x) => ok(x),
        Err(e) => api_err(e),
    }
}

async fn get_resource_definitions<B>(
    State(state): State<ApiState<B>>,
    Json(query): Json<GetResourceDefinitionFilter>,
) -> Response
where
    B: DawnstoreBackend + 'static,
{
    match state.backend.get_resource_definition(&query).await {
        Ok(x) => ok(x),
        Err(e) => api_err(e),
    }
}

async fn delete_object<B>(
    State(state): State<ApiState<B>>,
    Json(query): Json<DeleteObject>,
) -> Response
where
    B: DawnstoreBackend + 'static,
{
    match state.backend.delete(&query).await {
        Ok(()) => ok(true),
        Err(e) => api_err(e),
    }
}

// Keep the auth_err helper available for the JWT middleware to call if needed.
pub fn unauthorized(message: impl Into<String>) -> Response {
    auth_err(StatusCode::UNAUTHORIZED, message)
}
