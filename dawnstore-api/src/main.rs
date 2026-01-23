use std::sync::Arc;

use axum::Router;
use color_eyre::eyre;
use dawnstore_core::{
    backends::postgres::PostgresBackend,
    models::{Container, ForeignKey, ForeignKeyType},
};
use sqlx::PgPool;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt().init();
    let connection_string = std::env::var("DATABASE_URL")?;
    let pool = PgPool::connect(&connection_string).await?;
    let backend = PostgresBackend::new(pool);

    backend.sqlx_migrate().await?;

    backend
        .seed_object_schema::<Container>(
            "v2",
            "container",
            ["cont", "containers"],
            [ForeignKey::new(
                "parent",
                Some("children"),
                ForeignKeyType::OneOptional,
                Some("container"),
            )],
        )
        .await?;

    let backend = Arc::new(backend);

    let dawnstore_routes = dawnstore_core::controllers::get_dawnstore_default_routes(backend);
    let app = Router::new().merge(dawnstore_routes);

    let listener = TcpListener::bind("::0:8080").await.unwrap();
    tracing::info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await?;
    Ok(())
}
