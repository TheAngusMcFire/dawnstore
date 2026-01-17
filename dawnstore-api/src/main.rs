use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use color_eyre::eyre;
use dawnstore_core::{backends::postgres::PostgresBackend, models::EmptyObject};
use dawnstore_lib::*;
use sqlx::PgPool;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt().init();
    let connection_string = std::env::var("DATABASE_URL")?;
    let pool = PgPool::connect(&connection_string).await?;
    let backend = PostgresBackend::new(pool);
    backend
        .seed_object_schema::<EmptyObject>("v1", "empty", ["ep", "empties"])
        .await?;

    let app = Router::new()
        .route("/apply", post(apply))
        .route("/list", get(list))
        .route("/delete", delete(delete_object))
        .with_state(ApiState {
            backend: Arc::new(backend),
        });
    let listener = TcpListener::bind("::0:8080").await.unwrap();
    tracing::info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await?;

    Ok(())
}

#[derive(Clone)]
struct ApiState {
    backend: Arc<PostgresBackend>,
}

async fn apply(State(state): State<ApiState>, Json(obj): Json<serde_json::Value>) -> Response {
    match state.backend.apply_raw(obj).await {
        Ok(x) => Json(x).into_response(),
        Err(y) => format!("{y:?}").into_response(),
    }
}

async fn list(State(state): State<ApiState>, Query(query): Query<ListObjectsFilter>) -> Response {
    match state.backend.list(&query).await {
        Ok(x) => Json(x).into_response(),
        Err(y) => {
            let mut resp = format!("{y:?}").into_response();
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            resp
        }
    }
}

async fn delete_object(
    State(state): State<ApiState>,
    Query(query): Query<DeleteObject>,
) -> Response {
    match state.backend.delete(&query).await {
        Ok(x) => Json(x).into_response(),
        Err(y) => {
            let mut resp = format!("{y:?}").into_response();
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            resp
        }
    }
}
