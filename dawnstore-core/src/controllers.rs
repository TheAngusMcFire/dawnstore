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

// Manual Clone so the impl doesn't gain a spurious `B: Clone` bound.
// Arc<B> is always Clone regardless of B.
impl<B> Clone for ApiState<B> {
    fn clone(&self) -> Self {
        Self { backend: Arc::clone(&self.backend) }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

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

/// Return 404 if the requested kind is not registered, 400 otherwise.
fn error_response(err: DawnStoreError) -> Response {
    let status = match &err {
        DawnStoreError::UnknownResourceKind(_) => StatusCode::NOT_FOUND,
        DawnStoreError::NamespaceCanOnlyBeCreatedInSystemNamespace(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_REQUEST,
    };
    let mut resp = format!("{err}").into_response();
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
        return error_response(e);
    }
    match state.backend.apply_raw(obj).await {
        Ok(x) => Json(x).into_response(),
        Err(e) => error_response(e),
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
        // Unknown kind → 404
        let resolved = match state.backend.resource_cache().resolve(k) {
            Some(r) => r,
            None => return error_response(DawnStoreError::UnknownResourceKind(k.clone())),
        };
        // Namespace objects live in the system namespace; translate default → system.
        if resolved == "namespace"
            && matches!(query.namespace.as_deref(), None | Some("default"))
        {
            query.namespace = Some("system".to_string());
        }
    }
    match state.backend.get(&query).await {
        Ok(x) => Json(x).into_response(),
        Err(e) => error_response(e),
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
        // Unknown kind → 404
        let resolved = match state.backend.resource_cache().resolve(k) {
            Some(r) => r,
            None => return error_response(DawnStoreError::UnknownResourceKind(k.clone())),
        };
        if resolved == "namespace"
            && matches!(query.namespace.as_deref(), None | Some("default"))
        {
            query.namespace = Some("system".to_string());
        }
    }
    match state.backend.get_object_infos(&query).await {
        Ok(x) => Json(x).into_response(),
        Err(e) => error_response(e),
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
        Ok(x) => Json(x).into_response(),
        Err(e) => error_response(e),
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
        Ok(x) => Json(x).into_response(),
        Err(e) => error_response(e),
    }
}
