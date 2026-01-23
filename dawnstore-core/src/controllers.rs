use std::sync::Arc;

use crate::backends::postgres::PostgresBackend;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, post},
};
use dawnstore_lib::*;

pub fn get_dawnstore_default_routes(backend: Arc<PostgresBackend>) -> Router {
    Router::new()
        .route("/apply", post(apply))
        .route("/get-objects", post(get_objects))
        .route("/get-resource-definitions", post(get_resource_definitions))
        .route("/delete-object", delete(delete_object))
        .with_state(ApiState { backend })
}

#[derive(Clone)]
struct ApiState {
    backend: Arc<PostgresBackend>,
}

async fn apply(State(state): State<ApiState>, Json(obj): Json<serde_json::Value>) -> Response {
    match state.backend.apply_raw(obj).await {
        Ok(x) => Json(x).into_response(),
        Err(y) => {
            let mut resp = format!("{y}:{y:?}").into_response();
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            resp
        }
    }
}

async fn get_objects(
    State(state): State<ApiState>,
    Json(query): Json<GetObjectsFilter>,
) -> Response {
    match state.backend.get(&query).await {
        Ok(x) => Json(x).into_response(),
        Err(y) => {
            let mut resp = format!("{y:?}").into_response();
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            resp
        }
    }
}

async fn get_resource_definitions(
    State(state): State<ApiState>,
    Json(query): Json<GetResourceDefinitionFilter>,
) -> Response {
    match state.backend.get_resource_definition(&query).await {
        Ok(x) => Json(x).into_response(),
        Err(y) => {
            let mut resp = format!("{y:?}").into_response();
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            resp
        }
    }
}

async fn delete_object(State(state): State<ApiState>, Json(query): Json<DeleteObject>) -> Response {
    match state.backend.delete(&query).await {
        Ok(x) => Json(x).into_response(),
        Err(y) => {
            let mut resp = format!("{y:?}").into_response();
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            resp
        }
    }
}
