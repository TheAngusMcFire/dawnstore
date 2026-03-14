use std::sync::Arc;

use axum::Router;
use color_eyre::eyre;
use dawnstore_core::abstractions::{ForeignKey, ForeignKeyType};
use dawnstore_core::controllers::get_dawnstore_default_routes;
use dawnstore_postgres::PostgresBackend;
use tokio::net::TcpListener;

mod models;
use models::Container;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt().init();
    let connection_string = std::env::var("DATABASE_URL")?;
    let backend = PostgresBackend::new_from_connection_string(connection_string).await?;

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

    dawnstore_core::rbac::init(&backend).await?;
    backend.warm_caches().await?;

    let backend = Arc::new(backend);

    let public_key_pem = std::env::var("JWT_PUBLIC_KEY")?.into_bytes();

    let dawnstore_routes = get_dawnstore_default_routes(backend);
    let app = dawnstore_core::rbac::with_jwt_auth(
        Router::new().merge(dawnstore_routes),
        public_key_pem,
    );

    let listener = TcpListener::bind("::0:8080").await.unwrap();
    tracing::info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await?;
    Ok(())
}
