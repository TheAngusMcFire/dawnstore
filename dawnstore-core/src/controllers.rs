use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, post},
};
use dawnstore_lib::*;

use crate::abstractions::DawnstoreBackend;
use crate::error::DawnStoreError;
use crate::rbac::authz_service;
use crate::rbac::cache::{RbacCache, Verb};
use crate::rbac::constants::{
    KIND_GLOBAL_ROLE, KIND_GLOBAL_ROLE_BINDING, KIND_NAMESPACE, KIND_ROLE, KIND_ROLE_BINDING,
    SYSTEM_NAMESPACE,
};
use crate::rbac::helpers::object_string_id;
use crate::rbac::middleware::Claims;

pub fn get_dawnstore_default_routes<B>(backend: Arc<B>, rbac_cache: Arc<RbacCache>) -> Router
where
    B: DawnstoreBackend + 'static,
{
    Router::new()
        .route("/apply", post(apply::<B>))
        .route("/get-objects", post(get_objects::<B>))
        .route("/get-object-infos", post(get_object_infos::<B>))
        .route(
            "/get-resource-definitions",
            post(get_resource_definitions::<B>),
        )
        .route("/delete-object", delete(delete_object::<B>))
        .with_state(ApiState {
            backend,
            rbac_cache,
        })
}

struct ApiState<B> {
    backend: Arc<B>,
    rbac_cache: Arc<RbacCache>,
}

impl<B> Clone for ApiState<B> {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            rbac_cache: Arc::clone(&self.rbac_cache),
        }
    }
}

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
        if backend.resolve_kind(kind) == KIND_NAMESPACE {
            let ns = obj
                .get("namespace")
                .and_then(|n| n.as_str())
                .unwrap_or("default");
            if ns != SYSTEM_NAMESPACE {
                return Err(DawnStoreError::NamespaceCanOnlyBeCreatedInSystemNamespace(
                    ns.to_string(),
                ));
            }
        }
    }
    Ok(())
}

/// Extract `(namespace, kind, name)` from every object in the payload.
/// Used by the apply handler to check RBAC before touching the DB.
fn extract_apply_identities(value: &serde_json::Value) -> Vec<(String, String, String)> {
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

    objects
        .into_iter()
        .filter_map(|obj| {
            let kind = obj.get("kind")?.as_str()?.to_string();
            let name = obj.get("name")?.as_str()?.to_string();
            let ns = obj
                .get("namespace")
                .and_then(|n| n.as_str())
                .unwrap_or("default")
                .to_string();
            Some((ns, kind, name))
        })
        .collect()
}

/// Returns `true` if `kind` is an RBAC resource whose change should trigger
/// cache invalidation.
fn is_rbac_kind(kind: &str) -> bool {
    matches!(
        kind,
        KIND_ROLE | KIND_GLOBAL_ROLE | KIND_ROLE_BINDING | KIND_GLOBAL_ROLE_BINDING
    )
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

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn apply<B>(
    State(state): State<ApiState<B>>,
    claims_ext: Option<Extension<Claims>>,
    Json(obj): Json<serde_json::Value>,
) -> Response
where
    B: DawnstoreBackend + 'static,
{
    if let Err(e) = check_namespace_restriction(&*state.backend, &obj) {
        return api_err(e);
    }

    // RBAC: check Apply permission for every object in the payload (when JWT auth is active).
    if let Some(Extension(claims)) = &claims_ext {
        for (ns, kind, name) in extract_apply_identities(&obj) {
            let resolved_kind = state.backend.resolve_kind(&kind);

            // Check the caller can apply this object.
            match authz_service::is_allowed(
                &state.rbac_cache,
                &*state.backend,
                claims,
                Verb::Apply,
                &resolved_kind,
                &name,
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => return api_err(DawnStoreError::Forbidden),
                Err(e) => return api_err(e),
            }

            // Check the caller has Get access to every FK-referenced object.
            // We resolve the api_version from the object payload; fall back to wildcard.
            let api_version = obj
                .get("api_version")
                .or_else(|| {
                    // Array/list: find the matching object and read its api_version.
                    obj.as_array()
                        .and_then(|arr| {
                            arr.iter().find(|o| {
                                o.get("name").and_then(|n| n.as_str()) == Some(name.as_str())
                            })
                        })
                        .and_then(|o| o.get("api_version"))
                })
                .and_then(|v| v.as_str())
                .unwrap_or("*");

            // Extract the spec from the appropriate object in the payload.
            let spec_owner = if obj.is_array() {
                obj.as_array()
                    .and_then(|arr| {
                        arr.iter()
                            .find(|o| o.get("name").and_then(|n| n.as_str()) == Some(name.as_str()))
                    })
                    .cloned()
                    .unwrap_or(serde_json::Value::Null)
            } else {
                obj.clone()
            };

            let fk_refs = match state
                .backend
                .get_fk_refs(api_version, &resolved_kind, &spec_owner, &ns)
                .await
            {
                Ok(refs) => refs,
                Err(e) => return api_err(e),
            };

            for ref_sid in fk_refs {
                let parts: Vec<&str> = ref_sid.splitn(3, '/').collect();
                let (ref_kind, ref_name) = match parts.as_slice() {
                    [_, k, n] => (*k, *n),
                    _ => continue,
                };
                let resolved_ref_kind = state.backend.resolve_kind(ref_kind);
                match authz_service::is_allowed(
                    &state.rbac_cache,
                    &*state.backend,
                    claims,
                    Verb::Get,
                    &resolved_ref_kind,
                    ref_name,
                )
                .await
                {
                    Ok(true) => {}
                    Ok(false) => return api_err(DawnStoreError::Forbidden),
                    Err(e) => return api_err(e),
                }
            }
        }
    }

    match state.backend.apply_raw(obj).await {
        Ok(applied) => {
            // Invalidate RBAC cache for any applied RBAC resources.
            for obj in &applied {
                if is_rbac_kind(&obj.kind) {
                    state.rbac_cache.invalidate(&object_string_id(
                        &obj.namespace,
                        &obj.kind,
                        &obj.name,
                    ));
                }
            }
            ok(applied)
        }
        Err(e) => api_err(e),
    }
}

async fn get_objects<B>(
    State(state): State<ApiState<B>>,
    claims_ext: Option<Extension<Claims>>,
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
        if resolved == KIND_NAMESPACE
            && matches!(query.namespace.as_deref(), None | Some("default"))
        {
            query.namespace = Some(SYSTEM_NAMESPACE.to_string());
        }
    }

    // RBAC: restrict results to what the caller may read (when JWT auth is active).
    if let Some(Extension(claims)) = &claims_ext {
        match authz_service::allowed_scopes(&state.rbac_cache, &*state.backend, claims, Verb::Get)
            .await
        {
            Ok(allowed) => query.allowed = allowed,
            Err(e) => return api_err(e),
        }
    }

    match state.backend.get(&query).await {
        Ok(x) => ok(x),
        Err(e) => api_err(e),
    }
}

async fn get_object_infos<B>(
    State(state): State<ApiState<B>>,
    claims_ext: Option<Extension<Claims>>,
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
        if resolved == KIND_NAMESPACE
            && matches!(query.namespace.as_deref(), None | Some("default"))
        {
            query.namespace = Some(SYSTEM_NAMESPACE.to_string());
        }
    }

    // RBAC: restrict results to what the caller may read (when JWT auth is active).
    if let Some(Extension(claims)) = &claims_ext {
        match authz_service::allowed_scopes(&state.rbac_cache, &*state.backend, claims, Verb::Get)
            .await
        {
            Ok(allowed) => query.allowed = allowed,
            Err(e) => return api_err(e),
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
    claims_ext: Option<Extension<Claims>>,
    Json(query): Json<DeleteObject>,
) -> Response
where
    B: DawnstoreBackend + 'static,
{
    let resolved_kind = state.backend.resolve_kind(&query.kind);

    // RBAC: check Delete permission (when JWT auth is active).
    if let Some(Extension(claims)) = &claims_ext {
        match authz_service::is_allowed(
            &state.rbac_cache,
            &*state.backend,
            claims,
            Verb::Delete,
            &resolved_kind,
            &query.name,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => return api_err(DawnStoreError::Forbidden),
            Err(e) => return api_err(e),
        }
    }

    match state.backend.delete(&query).await {
        Ok(()) => {
            // Invalidate RBAC cache for deleted RBAC resources.
            if is_rbac_kind(&resolved_kind) {
                let ns = query.namespace.as_deref().unwrap_or("default");
                state
                    .rbac_cache
                    .invalidate(&object_string_id(ns, &resolved_kind, &query.name));
            }
            ok(true)
        }
        Err(e) => api_err(e),
    }
}

// Keep the auth_err helper available for the JWT middleware to call if needed.
pub fn unauthorized(message: impl Into<String>) -> Response {
    auth_err(StatusCode::UNAUTHORIZED, message)
}
