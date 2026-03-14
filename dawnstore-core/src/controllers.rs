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

async fn apply<B>(
    State(state): State<ApiState<B>>,
    Json(obj): Json<serde_json::Value>,
) -> Response
where
    B: DawnstoreBackend + 'static,
{
    match state.backend.apply_raw(obj).await {
        Ok(x) => Json(x).into_response(),
        Err(y) => {
            let mut resp = format!("{y}:{y:?}").into_response();
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            resp
        }
    }
}

async fn get_objects<B>(
    State(state): State<ApiState<B>>,
    Json(query): Json<GetObjectsFilter>,
) -> Response
where
    B: DawnstoreBackend + 'static,
{
    match state.backend.get(&query).await {
        Ok(x) => Json(x).into_response(),
        Err(y) => {
            let mut resp = format!("{y:?}").into_response();
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            resp
        }
    }
}

async fn get_object_infos<B>(
    State(state): State<ApiState<B>>,
    Json(query): Json<GetObjectInfosFilter>,
) -> Response
where
    B: DawnstoreBackend + 'static,
{
    match state.backend.get_object_infos(&query).await {
        Ok(x) => Json(x).into_response(),
        Err(y) => {
            let mut resp = format!("{y:?}").into_response();
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            resp
        }
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
        Err(y) => {
            let mut resp = format!("{y:?}").into_response();
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            resp
        }
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
        Err(y) => {
            let mut resp = format!("{y:?}").into_response();
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            resp
        }
    }
}
