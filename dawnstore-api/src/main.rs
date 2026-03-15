use std::sync::Arc;

use base64::prelude::*;
use color_eyre::eyre;
use dawnstore_core::abstractions::{ForeignKey, ForeignKeyBehaviour, ForeignKeyType};
use dawnstore_core::cache::DawnstoreCache;
use dawnstore_core::controllers::get_dawnstore_routes;
use dawnstore_postgres::PostgresBackend;
use tokio::net::TcpListener;

mod models;
use models::{Container, Deployment, Environment, Project, Secret, Team};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // ── CLI: --generate-keys ──────────────────────────────────────────────────
    if std::env::args().any(|a| a == "--generate-keys") {
        let keypair = dawnstore_core::rbac::jwt_service::generate_keypair()?;
        let private_b64 = BASE64_STANDARD.encode(&keypair.private_key_pem);
        let public_b64 = BASE64_STANDARD.encode(&keypair.public_key_pem);
        println!("JWT_PRIVATE_KEY_B64={private_b64}");
        println!("JWT_PUBLIC_KEY_B64={public_b64}");
        return Ok(());
    }

    // ── Server startup ────────────────────────────────────────────────────────
    tracing_subscriber::fmt().init();

    let private_key_pem = decode_key_env("JWT_PRIVATE_KEY_B64")?;
    let public_key_pem = decode_key_env("JWT_PUBLIC_KEY_B64")?;

    let connection_string = std::env::var("DATABASE_URL")?;
    let backend = PostgresBackend::new_from_connection_string(connection_string).await?;

    backend.sqlx_migrate().await?;

    // ── Schema registration ───────────────────────────────────────────────────
    //
    // Schemas must be registered before objects of that kind can be applied.
    // Register root kinds first so FK targets exist before FK sources.

    // Original demo model — a self-referencing container tree.
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

    // Team — root node, no FKs.
    backend
        .seed_object_schema::<Team>("v1", "team", ["teams", "tm"], [])
        .await?;

    // Project — belongs to one Team.
    backend
        .seed_object_schema::<Project>(
            "v1",
            "project",
            ["projects", "pj", "proj"],
            [ForeignKey::new(
                "team",
                None::<&str>,
                ForeignKeyType::One,
                Some("team"),
            )],
        )
        .await?;

    // Environment — belongs to one Project.
    backend
        .seed_object_schema::<Environment>(
            "v1",
            "environment",
            ["env", "environments", "envs"],
            [ForeignKey::new(
                "project",
                None::<&str>,
                ForeignKeyType::One,
                Some("project"),
            )],
        )
        .await?;

    // Secret — belongs to one Environment.
    backend
        .seed_object_schema::<Secret>(
            "v1",
            "secret",
            ["secrets", "sec"],
            [ForeignKey::new(
                "environment",
                None::<&str>,
                ForeignKeyType::One,
                Some("environment"),
            )],
        )
        .await?;

    // Deployment — references Project (One), Environment (One), and
    // zero-or-more Secrets (NoneOrMany). The secrets FK uses ForeignKeyBehaviour::Ignore
    // so that the FK walk does not block a deployment from being created before
    // its secrets exist — the array is informational ("this deployment needs these
    // secrets") rather than a hard existence constraint.
    backend
        .seed_object_schema::<Deployment>(
            "v1",
            "deployment",
            ["deployments", "dp", "deploy"],
            [
                ForeignKey::new("project", None::<&str>, ForeignKeyType::One, Some("project")),
                ForeignKey::new(
                    "environment",
                    None::<&str>,
                    ForeignKeyType::One,
                    Some("environment"),
                ),
                ForeignKey {
                    path: "secrets".into(),
                    parent_path: None,
                    ty: ForeignKeyType::NoneOrMany,
                    behaviour: ForeignKeyBehaviour::Fill,
                    foreign_kind: Some("secret".into()),
                },
            ],
        )
        .await?;

    dawnstore_core::rbac::init(&backend).await?;

    if let Some(token) = dawnstore_core::rbac::bootstrap(&backend, &private_key_pem).await? {
        println!("================================================================");
        println!("BOOTSTRAP TOKEN (printed once — store it securely):");
        println!("{token}");
        println!("================================================================");
    }

    let backend = Arc::new(backend);

    let cache = Arc::new(DawnstoreCache::init(&*backend).await?);

    let routes = get_dawnstore_routes(Arc::clone(&backend), Arc::clone(&cache), private_key_pem);

    let app = dawnstore_core::rbac::with_jwt_auth(routes, public_key_pem, Arc::clone(&cache));

    let listener = TcpListener::bind("::0:8080").await.unwrap();
    tracing::info!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await?;
    Ok(())
}

/// Decode a base64-encoded PEM key from an environment variable.
fn decode_key_env(var: &str) -> eyre::Result<Vec<u8>> {
    let b64 = std::env::var(var).map_err(|_| eyre::eyre!("missing env var {var}"))?;
    BASE64_STANDARD
        .decode(b64.trim())
        .map_err(|e| eyre::eyre!("{var}: invalid base64: {e}"))
}
