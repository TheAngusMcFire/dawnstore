//! End-to-end tests for dawnstore.
//!
//! Each test gets an isolated database via `#[sqlx::test]`.
//! Requires a running PostgreSQL instance:
//!
//!   docker compose up -d
//!   DATABASE_URL=postgres://postgres:password@localhost:5435/testing cargo test -p dawnstore-testing

use std::sync::Arc;

use axum::Router;
use dawnstore_client_lib::Api;
use dawnstore_core::abstractions::{ForeignKey, ForeignKeyType};
use dawnstore_core::controllers::get_dawnstore_default_routes;
use dawnstore_testing::Container;
use dawnstore_postgres::PostgresBackend;
use dawnstore_lib::{DeleteObject, GetObjectsFilter};
use sqlx::PgPool;
use tokio::net::TcpListener;

static MIGRATOR: sqlx::migrate::Migrator =
    sqlx::migrate!("../dawnstore-postgres/migrations");

/// Starts a test server backed by `pool`, seeds the `container` schema,
/// and returns a client pointed at it. The server runs until the test ends.
async fn spawn_server(pool: PgPool) -> Api {
    let backend = PostgresBackend::new(pool);
    backend
        .seed_object_schema::<Container>(
            "v1",
            "container",
            ["containers"],
            [ForeignKey::new(
                "parent",
                Some("children"),
                ForeignKeyType::OneOptional,
                Some("container"),
            )],
        )
        .await
        .unwrap();

    let backend = Arc::new(backend);
    let app = Router::new().merge(get_dawnstore_default_routes(backend));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    Api::new(base_url)
}

fn container(name: &str, nr: u32) -> String {
    serde_json::to_string(&serde_json::json!({
        "api_version": "v1",
        "kind": "container",
        "name": name,
        "nr": nr,
    }))
    .unwrap()
}

fn container_with_parent(name: &str, nr: u32, parent: &str) -> String {
    serde_json::to_string(&serde_json::json!({
        "api_version": "v1",
        "kind": "container",
        "name": name,
        "nr": nr,
        "parent": parent,
    }))
    .unwrap()
}

// ── Basic apply + get ────────────────────────────────────────────────────────

#[sqlx::test(migrator = "MIGRATOR")]
async fn apply_single_object_and_get(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    let result = api.apply_str(container("box-1", 42)).await.unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "box-1");
    assert_eq!(result[0].kind, "container");
    assert_eq!(result[0].api_version, "v1");
    assert_eq!(result[0].namespace, "default");
    assert_eq!(result[0].spec["nr"], 42);

    let objects = api
        .get_objects(&GetObjectsFilter {
            kind: Some("container".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].name, "box-1");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn apply_is_idempotent(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    api.apply_str(container("box-1", 1)).await.unwrap();
    api.apply_str(container("box-1", 99)).await.unwrap();

    let objects = api
        .get_objects(&GetObjectsFilter {
            kind: Some("container".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(objects.len(), 1, "second apply should update, not insert");
    assert_eq!(objects[0].spec["nr"], 99);

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn apply_json_array(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    let arr = serde_json::to_string(&serde_json::json!([
        {"api_version": "v1", "kind": "container", "name": "box-a", "nr": 1},
        {"api_version": "v1", "kind": "container", "name": "box-b", "nr": 2},
    ]))
    .unwrap();

    let result = api.apply_str(arr).await.unwrap();
    assert_eq!(result.len(), 2);

    let objects = api
        .get_objects(&GetObjectsFilter {
            kind: Some("container".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(objects.len(), 2);

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn apply_list_wrapper(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    let list = serde_json::to_string(&serde_json::json!({
        "kind": "List",
        "object_kind": "container",
        "object_api_version": "v1",
        "list": [
            {"name": "box-a", "nr": 1},
            {"name": "box-b", "nr": 2},
        ]
    }))
    .unwrap();

    let result = api.apply_str(list).await.unwrap();
    assert_eq!(result.len(), 2);

    Ok(())
}

// ── Filtering ────────────────────────────────────────────────────────────────

#[sqlx::test(migrator = "MIGRATOR")]
async fn get_filter_by_name(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    api.apply_str(container("alpha", 1)).await.unwrap();
    api.apply_str(container("beta", 2)).await.unwrap();

    let objects = api
        .get_objects(&GetObjectsFilter {
            kind: Some("container".into()),
            name: Some("alpha".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].name, "alpha");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn get_filter_by_namespace(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    api.apply_str(
        serde_json::to_string(&serde_json::json!({
            "api_version": "v1", "kind": "container",
            "namespace": "ns-a", "name": "box-1", "nr": 1,
        }))
        .unwrap(),
    )
    .await
    .unwrap();

    api.apply_str(
        serde_json::to_string(&serde_json::json!({
            "api_version": "v1", "kind": "container",
            "namespace": "ns-b", "name": "box-2", "nr": 2,
        }))
        .unwrap(),
    )
    .await
    .unwrap();

    let objects = api
        .get_objects(&GetObjectsFilter {
            namespace: Some("ns-a".into()),
            kind: Some("container".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].name, "box-1");

    Ok(())
}

// ── Delete ───────────────────────────────────────────────────────────────────

#[sqlx::test(migrator = "MIGRATOR")]
async fn delete_removes_object(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    api.apply_str(container("box-1", 1)).await.unwrap();
    api.delete_object(&DeleteObject {
        namespace: None,
        kind: "container".into(),
        name: "box-1".into(),
    })
    .await
    .unwrap();

    let objects = api
        .get_objects(&GetObjectsFilter {
            kind: Some("container".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert!(objects.is_empty());

    Ok(())
}

// ── Resource definitions ─────────────────────────────────────────────────────

#[sqlx::test(migrator = "MIGRATOR")]
async fn resource_definitions_returns_seeded_schema(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    let defs = api
        .get_resource_definitions(&Default::default())
        .await
        .unwrap();

    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].kind, "container");
    assert_eq!(defs[0].api_version, "v1");
    assert!(defs[0].aliases.contains(&"containers".to_string()));

    Ok(())
}

// ── Schema validation ────────────────────────────────────────────────────────

#[sqlx::test(migrator = "MIGRATOR")]
async fn schema_validation_rejects_wrong_field_type(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    let bad = serde_json::to_string(&serde_json::json!({
        "api_version": "v1", "kind": "container",
        "name": "bad-box", "nr": "not-a-number",
    }))
    .unwrap();

    assert!(api.apply_str(bad).await.is_err());

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn schema_validation_rejects_unknown_fields(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    let bad = serde_json::to_string(&serde_json::json!({
        "api_version": "v1", "kind": "container",
        "name": "bad-box", "nr": 1, "unexpected_field": "oops",
    }))
    .unwrap();

    assert!(api.apply_str(bad).await.is_err());

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn apply_unregistered_kind_fails(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    let bad = serde_json::to_string(&serde_json::json!({
        "api_version": "v1", "kind": "unregistered",
        "name": "box-1", "nr": 1,
    }))
    .unwrap();

    assert!(api.apply_str(bad).await.is_err());

    Ok(())
}

// ── Foreign keys ─────────────────────────────────────────────────────────────

#[sqlx::test(migrator = "MIGRATOR")]
async fn foreign_key_valid_reference(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    api.apply_str(container("parent-box", 1)).await.unwrap();
    api.apply_str(container_with_parent("child-box", 2, "parent-box"))
        .await
        .unwrap();

    let objects = api
        .get_objects(&GetObjectsFilter {
            kind: Some("container".into()),
            name: Some("child-box".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].spec["parent"], "parent-box");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn foreign_key_object_filled_on_get(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    api.apply_str(container("parent-box", 10)).await.unwrap();
    api.apply_str(container_with_parent("child-box", 20, "parent-box"))
        .await
        .unwrap();

    let objects = api
        .get_objects(&GetObjectsFilter {
            kind: Some("container".into()),
            name: Some("child-box".into()),
            fill_child_foreign_keys: true,
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(objects.len(), 1);
    let parent_obj = &objects[0].spec["parent_object"];
    assert!(parent_obj.is_object(), "parent_object should be populated");
    assert_eq!(parent_obj["name"], "parent-box");
    assert_eq!(parent_obj["nr"], 10);

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn foreign_key_rejects_nonexistent_reference(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    let result = api
        .apply_str(container_with_parent("child-box", 1, "ghost-parent"))
        .await;

    assert!(result.is_err());

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn foreign_key_updated_on_reapply(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    api.apply_str(container("parent-a", 1)).await.unwrap();
    api.apply_str(container("parent-b", 2)).await.unwrap();
    api.apply_str(container_with_parent("child", 3, "parent-a"))
        .await
        .unwrap();

    // re-apply child pointing at a different parent
    api.apply_str(container_with_parent("child", 3, "parent-b"))
        .await
        .unwrap();

    let objects = api
        .get_objects(&GetObjectsFilter {
            kind: Some("container".into()),
            name: Some("child".into()),
            fill_child_foreign_keys: true,
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(objects[0].spec["parent_object"]["name"], "parent-b");

    Ok(())
}
