use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    response::IntoResponse,
    routing::{delete, get, post},
};
use color_eyre::eyre;
use serde::Deserialize;
use sqlx::PgPool;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt().init();
    let connection_string = std::env::var("CONNECTION_STRING")?;
    let pool = Arc::new(PgPool::connect(&connection_string).await?);
    dawnstore_core::backends::postgres::sqlx_migrate(pool.as_ref()).await?;

    let app = Router::new()
        .route("/apply", post(apply))
        .route("/list", get(list))
        .route("/delete", delete(delete_object))
        .with_state(ApiState { pool });
    let listener = TcpListener::bind("::0:8080").await.unwrap();
    tracing::info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await?;

    Ok(())
}

#[derive(Clone)]
struct ApiState {
    pool: Arc<PgPool>,
}

async fn apply(State(state): State<ApiState>) -> impl IntoResponse {}

#[derive(Deserialize)]
struct ListObject {
    pub namespace: Option<String>,
    pub kind: String,
    pub name: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}
async fn list(
    State(state): State<ApiState>,
    Query(query): Query<ListObject>,
    Json(obj): Json<serde_json::Value>,
) -> impl IntoResponse {
}

#[derive(Deserialize)]
struct DeleteObject {
    pub namespace: Option<String>,
    pub kind: String,
    pub name: String,
}
async fn delete_object(
    State(state): State<ApiState>,
    Query(query): Query<DeleteObject>,
) -> impl IntoResponse {
}
