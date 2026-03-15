//! End-to-end tests for dawnstore.
//!
//! Each test gets an isolated database via `#[sqlx::test]`.
//! Requires a running PostgreSQL instance:
//!
//!   docker compose up -d
//!   DATABASE_URL=postgres://postgres:password@localhost:5435/testing cargo test -p dawnstore-testing

use std::sync::Arc;

use dawnstore_client_lib::Api;
use dawnstore_core::abstractions::{ForeignKey, ForeignKeyType};
use dawnstore_core::cache::DawnstoreCache;
use dawnstore_core::controllers::get_dawnstore_routes;
use dawnstore_testing::Container;
use dawnstore_postgres::PostgresBackend;
use dawnstore_lib::{DeleteObject, GetObjectsFilter};
use sqlx::PgPool;
use tokio::net::TcpListener;

static MIGRATOR: sqlx::migrate::Migrator =
    sqlx::migrate!("../dawnstore-postgres/migrations");

/// Starts a test server backed by `pool`, seeds RBAC schemas + the `container`
/// schema, and returns a client pointed at it. The server runs until the test ends.
///
/// RBAC schemas (including the `namespace` kind) are always seeded so that the
/// namespace-existence check is active in every test. A `default` namespace
/// object is pre-created so existing tests that apply to "default" work without
/// explicitly creating it.
async fn spawn_server(pool: PgPool) -> Api {
    let backend = PostgresBackend::new(pool);

    // Seed RBAC schemas + system/default namespaces + superadmin SA.
    dawnstore_core::rbac::init(&backend).await.unwrap();

    // Seed the container schema.
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
    let cache = Arc::new(DawnstoreCache::init(&*backend).await.unwrap());
    let app = get_dawnstore_routes(backend, cache, vec![]);
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

    // Create namespaces first so the namespace-existence check passes.
    api.apply_str(serde_json::to_string(&serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "ns-a"},
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "ns-b"},
    ])).unwrap()).await.unwrap();

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

    // RBAC schemas are also seeded; filter down to the container schema.
    let container_def = defs.iter().find(|d| d.kind == "container" && d.api_version == "v1");
    assert!(container_def.is_some(), "container schema must be present");
    let container_def = container_def.unwrap();
    assert_eq!(container_def.kind, "container");
    assert_eq!(container_def.api_version, "v1");
    assert!(container_def.aliases.contains(&"containers".to_string()));

    Ok(())
}

// ── Alias resolution ─────────────────────────────────────────────────────────

#[sqlx::test(migrator = "MIGRATOR")]
async fn get_objects_by_alias(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    api.apply_str(container("alias-box", 42)).await.unwrap();

    // "containers" is a registered alias for "container"
    let by_alias = api
        .get_objects(&GetObjectsFilter {
            kind: Some("containers".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    let by_kind = api
        .get_objects(&GetObjectsFilter {
            kind: Some("container".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(by_alias.len(), 1);
    assert_eq!(by_alias[0].name, "alias-box");
    assert_eq!(by_alias[0].spec["nr"], 42);
    assert_eq!(by_alias.len(), by_kind.len());
    assert_eq!(by_alias[0].id, by_kind[0].id);

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn aliases_resolve_independently_for_multiple_kinds(pool: PgPool) -> sqlx::Result<()> {
    // Seed two kinds each with their own alias so we can verify alias resolution
    // works correctly when multiple kinds share the same server.
    use dawnstore_testing::EmptyObject;

    let backend = PostgresBackend::new(pool);
    dawnstore_core::rbac::init(&backend).await.unwrap();
    backend
        .seed_object_schema::<Container>(
            "v1",
            "container",
            ["containers", "c"],
            [ForeignKey::new(
                "parent",
                Some("children"),
                ForeignKeyType::OneOptional,
                Some("container"),
            )],
        )
        .await
        .unwrap();
    backend
        .seed_object_schema::<EmptyObject>("v1", "widget", ["widgets", "w"], [])
        .await
        .unwrap();

    let backend = Arc::new(backend);
    let cache = Arc::new(DawnstoreCache::init(&*backend).await.unwrap());
    let app = get_dawnstore_routes(backend, cache, vec![]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let api = Api::new(base_url);

    api.apply_str(container("box-a", 1)).await.unwrap();
    api.apply_str(
        serde_json::to_string(&serde_json::json!({
            "api_version": "v1",
            "kind": "widget",
            "name": "gadget-1",
        }))
        .unwrap(),
    )
    .await
    .unwrap();

    // Both canonical aliases ("c" and "w") resolve to the correct kind.
    let containers = api
        .get_objects(&GetObjectsFilter {
            kind: Some("c".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let widgets = api
        .get_objects(&GetObjectsFilter {
            kind: Some("w".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(containers.len(), 1);
    assert_eq!(containers[0].kind, "container");
    assert_eq!(containers[0].name, "box-a");

    assert_eq!(widgets.len(), 1);
    assert_eq!(widgets[0].kind, "widget");
    assert_eq!(widgets[0].name, "gadget-1");

    // Alias for one kind must not match objects of another kind.
    let containers_via_long = api
        .get_objects(&GetObjectsFilter {
            kind: Some("containers".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let widgets_via_long = api
        .get_objects(&GetObjectsFilter {
            kind: Some("widgets".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(containers_via_long.len(), 1);
    assert_eq!(widgets_via_long.len(), 1);
    assert_ne!(containers_via_long[0].id, widgets_via_long[0].id);

    Ok(())
}

// ── Controller rules ─────────────────────────────────────────────────────────

#[sqlx::test(migrator = "MIGRATOR")]
async fn get_objects_unknown_kind_returns_error(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    let result = api
        .get_objects(&GetObjectsFilter {
            kind: Some("ghost".into()),
            ..Default::default()
        })
        .await;

    assert!(result.is_err(), "unknown kind should return an error");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn namespace_created_outside_system_is_rejected(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    let resp = server
        .api
        .get_client()
        .post(format!("{}/apply", server.api.get_base_url()))
        .bearer_auth(&server.bootstrap_token)
        .header("content-type", "application/json")
        .body(
            serde_json::json!({
                "api_version": "v1",
                "kind": "namespace",
                "namespace": "other",
                "name": "my-namespace"
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();

    // Controller now always returns HTTP 200; errors are in the envelope body.
    assert!(resp.status().is_success());
    let body: dawnstore_lib::DawnStoreResponse<serde_json::Value> = resp.json().await.unwrap();
    assert!(body.error.is_some(), "namespace in non-system namespace must produce an error");
    assert!(body.data.is_none());

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn namespace_query_translates_default_to_system(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    // rbac::init seeds a `system/namespace/system` object.
    // Querying with namespace="default" must transparently return it.
    let resp = server
        .api
        .get_client()
        .post(format!("{}/get-objects", server.api.get_base_url()))
        .bearer_auth(&server.bootstrap_token)
        .json(&serde_json::json!({ "kind": "namespace", "namespace": "default" }))
        .send()
        .await
        .unwrap();

    assert!(resp.status().is_success());
    let body: dawnstore_lib::DawnStoreResponse<Vec<serde_json::Value>> =
        resp.json().await.unwrap();
    let objects = body.data.expect("expected data in response");
    // rbac::init seeds both "system" and "default" namespace objects (both live in
    // the system namespace), so namespace="default" (→ "system") returns both.
    assert_eq!(objects.len(), 2, "should find both seeded namespaces");
    assert!(
        objects.iter().any(|o| o["name"] == "system" && o["namespace"] == "system"),
        "system namespace must be present"
    );

    // Querying with no namespace should also return system namespaces.
    let resp2 = server
        .api
        .get_client()
        .post(format!("{}/get-objects", server.api.get_base_url()))
        .bearer_auth(&server.bootstrap_token)
        .json(&serde_json::json!({ "kind": "namespace" }))
        .send()
        .await
        .unwrap();

    assert!(resp2.status().is_success());
    let body2: dawnstore_lib::DawnStoreResponse<Vec<serde_json::Value>> =
        resp2.json().await.unwrap();
    let objects2 = body2.data.expect("expected data in response");
    assert_eq!(objects2.len(), 2);
    assert!(objects2.iter().any(|o| o["name"] == "system"));

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

// ── Nav-prop security (Issue 9) ───────────────────────────────────────────────

/// Embedded nav-prop round-trip: a container sent with a `parent_object` payload
/// (as returned by fill_child_foreign_keys) must be correctly extracted and both
/// objects stored — the original behaviour must be preserved.
#[sqlx::test(migrator = "MIGRATOR")]
async fn nav_prop_round_trip_still_works(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    // Apply parent first so it exists.
    api.apply_str(container("parent-box", 10)).await.unwrap();

    // Apply child with a fully-embedded parent_object (round-trip scenario).
    // Note: ObjectAny uses #[serde(flatten)] for spec, so container fields (nr)
    // live at the top level of the object, not nested under a "spec" key.
    let result = api
        .apply_str(serde_json::to_string(&serde_json::json!({
            "api_version": "v1",
            "kind": "container",
            "name": "child-box",
            "nr": 20,
            "parent": "parent-box",
            "parent_object": {
                "api_version": "v1",
                "kind": "container",
                "name": "parent-box",
                "nr": 10
            }
        })).unwrap())
        .await;
    assert!(result.is_ok(), "nav-prop round-trip apply should succeed: {result:?}");

    Ok(())
}

/// A registered nav-prop field with a value that is NOT a valid ObjectAny must
/// return an error instead of silently discarding the data.
#[sqlx::test(migrator = "MIGRATOR")]
async fn nav_prop_invalid_value_returns_error(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    // `parent_object` is a registered nav-prop (from the `parent` FK constraint).
    // Sending a plain string instead of an ObjectAny must be rejected.
    let result = api
        .apply_str(serde_json::to_string(&serde_json::json!({
            "api_version": "v1",
            "kind": "container",
            "name": "test-box",
            "nr": 1,
            "parent_object": "this is not an object"
        })).unwrap())
        .await;
    assert!(result.is_err(), "invalid nav-prop value must return an error, not silently drop data");

    Ok(())
}

/// A spec field that ends with `_object` but is NOT a registered nav-prop for
/// this kind must stay in the spec (where it fails schema validation for kinds
/// with deny_unknown_fields), not be silently extracted and applied.
#[sqlx::test(migrator = "MIGRATOR")]
async fn unregistered_object_field_is_not_extracted(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    // `grandparent_object` is not declared as a FK constraint for Container.
    // Before the fix this would be silently extracted and the embedded object
    // applied as a side-effect, bypassing schema validation.
    // After the fix the field stays in the spec, which fails schema validation
    // (Container uses deny_unknown_fields).
    let result = api
        .apply_str(serde_json::to_string(&serde_json::json!({
            "api_version": "v1",
            "kind": "container",
            "name": "test-box",
            "nr": 1,
            "grandparent_object": {
                "api_version": "v1",
                "kind": "container",
                "name": "smuggled-object",
                "spec": { "nr": 99 }
            }
        })).unwrap())
        .await;
    assert!(result.is_err(), "unregistered _object field must fail schema validation, not be silently extracted");

    // Confirm the smuggled object was NOT persisted.
    let objects = api
        .get_objects(&GetObjectsFilter {
            kind: Some("container".into()),
            name: Some("smuggled-object".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(objects.is_empty(), "smuggled object must not have been persisted: {objects:?}");

    Ok(())
}

// ── RBAC ─────────────────────────────────────────────────────────────────────
//
// Tests for rbac::init (seeding), rbac::bootstrap, and /rbac/issue-token.
// Seeding / bootstrap tests run directly against the backend; HTTP-layer tests
// use a full server with JWT middleware.

use dawnstore_core::rbac::jwt_service;

#[allow(dead_code)]
struct RbacTestServer {
    api: Api,
    bootstrap_token: String,
    public_key_pem: Vec<u8>,
    private_key_pem: Vec<u8>,
}

async fn spawn_rbac_server(pool: PgPool) -> RbacTestServer {
    let keypair = jwt_service::generate_keypair().unwrap();
    let backend = PostgresBackend::new(pool);

    dawnstore_core::rbac::init(&backend).await.unwrap();

    // Also seed the container schema so RBAC enforcement tests can use it.
    backend
        .seed_object_schema::<Container>(
            "v2",
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

    let bootstrap_token =
        dawnstore_core::rbac::bootstrap(&backend, &keypair.private_key_pem)
            .await
            .unwrap()
            .expect("first startup must return a bootstrap token");

    let backend = Arc::new(backend);
    let cache = Arc::new(DawnstoreCache::init(&*backend).await.unwrap());
    let routes = get_dawnstore_routes(
        Arc::clone(&backend),
        Arc::clone(&cache),
        keypair.private_key_pem.clone(),
    );
    let app = dawnstore_core::rbac::with_jwt_auth(
        routes,
        keypair.public_key_pem.clone(),
        Arc::clone(&cache),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    RbacTestServer {
        api: Api::new(base_url),
        bootstrap_token,
        public_key_pem: keypair.public_key_pem,
        private_key_pem: keypair.private_key_pem,
    }
}

// ── Unit: authz_service ───────────────────────────────────────────────────────

#[test]
fn authz_is_superadmin_accepts_only_system_superadmin() {
    use dawnstore_core::cache::is_superadmin;
    use dawnstore_core::rbac::middleware::Claims;

    let yes = Claims {
        sub: "superadmin".into(),
        namespace: "system".into(),
        token_name: "bootstrap".into(),
        token_id: uuid::Uuid::new_v4(),
        exp: u64::MAX,
    };
    assert!(is_superadmin(&yes));

    for (sub, ns) in [("superadmin", "other"), ("other", "system"), ("other", "other")] {
        let no = Claims {
            sub: sub.into(),
            namespace: ns.into(),
            token_name: "t".into(),
            token_id: uuid::Uuid::new_v4(),
            exp: u64::MAX,
        };
        assert!(!is_superadmin(&no), "should not be superadmin: {sub}@{ns}");
    }
}

// ── Seeding ───────────────────────────────────────────────────────────────────

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_init_seeds_system_namespace(pool: PgPool) -> sqlx::Result<()> {
    let backend = PostgresBackend::new(pool);
    dawnstore_core::rbac::init(&backend).await.unwrap();

    use dawnstore_core::abstractions::DawnstoreBackend as _;
    let obj = backend.get_object("system", "namespace", "system").await.unwrap();

    assert!(obj.is_some());
    let obj = obj.unwrap();
    assert_eq!(obj.name, "system");
    assert_eq!(obj.namespace, "system");
    assert_eq!(obj.kind, "namespace");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_init_seeds_superadmin(pool: PgPool) -> sqlx::Result<()> {
    let backend = PostgresBackend::new(pool);
    dawnstore_core::rbac::init(&backend).await.unwrap();

    use dawnstore_core::abstractions::DawnstoreBackend as _;
    let obj = backend.get_object("system", "serviceaccount", "superadmin").await.unwrap();

    assert!(obj.is_some());
    let obj = obj.unwrap();
    assert_eq!(obj.name, "superadmin");
    assert_eq!(obj.namespace, "system");

    Ok(())
}

// ── Bootstrap ─────────────────────────────────────────────────────────────────

#[sqlx::test(migrator = "MIGRATOR")]
async fn bootstrap_creates_token_on_first_startup(pool: PgPool) -> sqlx::Result<()> {
    let keypair = jwt_service::generate_keypair().unwrap();
    let backend = PostgresBackend::new(pool);
    dawnstore_core::rbac::init(&backend).await.unwrap();

    let token = dawnstore_core::rbac::bootstrap(&backend, &keypair.private_key_pem)
        .await
        .unwrap();

    assert!(token.is_some(), "first startup should produce a token");

    // The returned JWT must be valid and carry superadmin claims.
    let jwt = token.unwrap();
    let claims = jwt_service::validate_token(&jwt, &keypair.public_key_pem).unwrap();
    assert_eq!(claims.sub, "superadmin");
    assert_eq!(claims.namespace, "system");
    assert_eq!(claims.token_name, "bootstrap");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bootstrap_is_noop_on_second_call(pool: PgPool) -> sqlx::Result<()> {
    let keypair = jwt_service::generate_keypair().unwrap();
    let backend = PostgresBackend::new(pool);
    dawnstore_core::rbac::init(&backend).await.unwrap();

    let first = dawnstore_core::rbac::bootstrap(&backend, &keypair.private_key_pem)
        .await
        .unwrap();
    assert!(first.is_some());

    let second = dawnstore_core::rbac::bootstrap(&backend, &keypair.private_key_pem)
        .await
        .unwrap();
    assert!(second.is_none(), "second call must be a no-op");

    Ok(())
}

// ── HTTP: /rbac/issue-token ───────────────────────────────────────────────────

#[sqlx::test(migrator = "MIGRATOR")]
async fn issue_token_rejects_unauthenticated(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    let resp = server
        .api
        .get_client()
        .post(format!("{}/rbac/issue-token", server.api.get_base_url()))
        // no Authorization header
        .json(&serde_json::json!({
            "namespace": "system",
            "service_account": "superadmin",
            "token_name": "test-token"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 401);

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn issue_token_superadmin_can_issue(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    // Issue a new token for the superadmin itself (already seeded, no extra setup needed).
    let resp = server
        .api
        .get_client()
        .post(format!("{}/rbac/issue-token", server.api.get_base_url()))
        .bearer_auth(&server.bootstrap_token)
        .json(&serde_json::json!({
            "namespace": "system",
            "service_account": "superadmin",
            "token_name": "integration-test-token"
        }))
        .send()
        .await
        .unwrap();

    assert!(resp.status().is_success(), "status: {}", resp.status());

    let body: serde_json::Value = resp.json().await.unwrap();
    let data = &body["data"];
    let jwt = data["token"].as_str().expect("response must have a token field");
    let token_id_str = data["token_id"].as_str().expect("response must have a token_id field");
    assert!(!jwt.is_empty());
    assert!(!token_id_str.is_empty());

    // The issued JWT must validate correctly with the server's public key.
    let claims = jwt_service::validate_token(jwt, &server.public_key_pem).unwrap();
    assert_eq!(claims.sub, "superadmin");
    assert_eq!(claims.namespace, "system");
    assert_eq!(claims.token_name, "integration-test-token");
    assert_eq!(claims.token_id.to_string(), token_id_str);

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn issue_token_non_superadmin_is_forbidden(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    // Create "test-ns" namespace and a regular service account in it.
    let apply_resp = server
        .api
        .get_client()
        .post(format!("{}/apply", server.api.get_base_url()))
        .bearer_auth(&server.bootstrap_token)
        .header("content-type", "application/json")
        .body(
            serde_json::json!([
                {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "test-ns"},
                {"api_version": "v1", "kind": "serviceaccount", "namespace": "test-ns", "name": "regular"}
            ])
            .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert!(apply_resp.status().is_success(), "failed to create service account");

    // Superadmin issues a token for the regular service account.
    let issue_resp = server
        .api
        .get_client()
        .post(format!("{}/rbac/issue-token", server.api.get_base_url()))
        .bearer_auth(&server.bootstrap_token)
        .json(&serde_json::json!({
            "namespace": "test-ns",
            "service_account": "regular",
            "token_name": "regular-token"
        }))
        .send()
        .await
        .unwrap();
    assert!(issue_resp.status().is_success(), "superadmin should be able to issue token");

    let regular_jwt = issue_resp.json::<serde_json::Value>().await.unwrap();
    let regular_jwt = regular_jwt["data"]["token"].as_str().unwrap().to_string();

    // The regular token must itself be cryptographically valid.
    jwt_service::validate_token(&regular_jwt, &server.public_key_pem)
        .expect("issued token must be valid");

    // Using the regular token to issue yet another token must be rejected with 403.
    let forbidden_resp = server
        .api
        .get_client()
        .post(format!("{}/rbac/issue-token", server.api.get_base_url()))
        .bearer_auth(&regular_jwt)
        .json(&serde_json::json!({
            "namespace": "test-ns",
            "service_account": "regular",
            "token_name": "another-token"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(forbidden_resp.status().as_u16(), 403, "non-superadmin must receive 403");

    Ok(())
}

/// Issue-token must reject requests that reference a non-existent ServiceAccount.
/// Previously `issue_token` called `upsert_objects` directly and bypassed FK
/// validation, so a token could be issued for a SA that did not exist.
#[sqlx::test(migrator = "MIGRATOR")]
async fn issue_token_rejects_nonexistent_service_account(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    let resp = server
        .api
        .get_client()
        .post(format!("{}/rbac/issue-token", server.api.get_base_url()))
        .bearer_auth(&server.bootstrap_token)
        .json(&serde_json::json!({
            "namespace": "system",
            "service_account": "ghost",      // does not exist
            "token_name": "ghost-token"
        }))
        .send()
        .await
        .unwrap();

    assert!(resp.status().is_success(), "response envelope status: {}", resp.status());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(!body["error"].is_null(), "expected an error for non-existent SA: {body}");

    // Confirm no token object was persisted.
    let r = http_get_objects(&server, &server.bootstrap_token, serde_json::json!({
        "namespace": "system", "kind": "serviceaccounttoken", "name": "ghost-token"
    })).await;
    assert!(r["error"].is_null(), "get must not error: {r}");
    let items = get_resp_data_array(&r);
    assert!(items.is_empty(), "token object must not have been created: {r}");

    Ok(())
}

/// Object names containing '/' must be rejected to avoid ambiguous string IDs.
#[sqlx::test(migrator = "MIGRATOR")]
async fn apply_rejects_name_with_slash(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    let resp = http_apply(&server, &server.bootstrap_token, serde_json::json!({
        "apiVersion": "v1",
        "kind": "Namespace",
        "namespace": "system",
        "name": "bad/name"
    })).await;

    assert!(!resp["error"].is_null(), "expected error for name with '/': {resp}");

    Ok(())
}

/// Object namespaces containing '/' must be rejected to avoid ambiguous string IDs.
#[sqlx::test(migrator = "MIGRATOR")]
async fn apply_rejects_namespace_with_slash(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    let resp = http_apply(&server, &server.bootstrap_token, serde_json::json!({
        "apiVersion": "v1",
        "kind": "Namespace",
        "namespace": "bad/ns",
        "name": "my-ns"
    })).await;

    assert!(!resp["error"].is_null(), "expected error for namespace with '/': {resp}");

    Ok(())
}

/// `issue_token` must also reject token names / SA names / namespaces with '/'.
#[sqlx::test(migrator = "MIGRATOR")]
async fn issue_token_rejects_slash_in_name(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    // First create a valid SA to be sure the rejection isn't hitting the SA check.
    let sa_resp = http_apply(&server, &server.bootstrap_token, serde_json::json!({
        "api_version": "v1",
        "kind": "serviceaccount",
        "namespace": "system",
        "name": "valid-sa"
    })).await;
    assert!(sa_resp["error"].is_null(), "SA creation must succeed: {sa_resp}");

    // Token name with slash.
    let resp = server
        .api
        .get_client()
        .post(format!("{}/rbac/issue-token", server.api.get_base_url()))
        .bearer_auth(&server.bootstrap_token)
        .json(&serde_json::json!({
            "namespace": "system",
            "service_account": "valid-sa",
            "token_name": "bad/token"
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(!body["error"].is_null(), "expected error for token name with '/': {body}");

    // Namespace with slash.
    let resp2 = server
        .api
        .get_client()
        .post(format!("{}/rbac/issue-token", server.api.get_base_url()))
        .bearer_auth(&server.bootstrap_token)
        .json(&serde_json::json!({
            "namespace": "bad/ns",
            "service_account": "valid-sa",
            "token_name": "ok-token"
        }))
        .send()
        .await
        .unwrap();
    assert!(resp2.status().is_success());
    let body2: serde_json::Value = resp2.json().await.unwrap();
    assert!(!body2["error"].is_null(), "expected error for namespace with '/': {body2}");

    Ok(())
}

// ── JWT authentication negative cases ────────────────────────────────────────

/// Helper: POST to any protected endpoint with the given raw Authorization value.
async fn get_objects_with_auth(server: &RbacTestServer, auth_header: &str) -> u16 {
    server
        .api
        .get_client()
        .post(format!("{}/get-objects", server.api.get_base_url()))
        .header("Authorization", auth_header)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn auth_rejects_malformed_token(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    assert_eq!(get_objects_with_auth(&server, "Bearer not.a.jwt").await, 401);
    assert_eq!(get_objects_with_auth(&server, "Bearer ").await, 401);
    assert_eq!(get_objects_with_auth(&server, "Bearer eyJhbGciOiJFUzM4NCJ9.garbage.sig").await, 401);

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn auth_rejects_token_signed_with_wrong_key(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    // Sign a structurally valid token with a *different* keypair.
    let other_keypair = jwt_service::generate_keypair().unwrap();
    let forged = jwt_service::create_token(
        "superadmin",
        "system",
        "forged",
        uuid::Uuid::new_v4(),
        chrono::Utc::now() + chrono::Duration::hours(1),
        &other_keypair.private_key_pem,
    )
    .unwrap();

    assert_eq!(
        get_objects_with_auth(&server, &format!("Bearer {forged}")).await,
        401,
        "token signed by wrong key must be rejected"
    );

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn auth_rejects_expired_token(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    // Create a token that expired 1 hour ago.
    let expired = jwt_service::create_token(
        "superadmin",
        "system",
        "expired",
        uuid::Uuid::new_v4(),
        chrono::Utc::now() - chrono::Duration::hours(1),
        &server.private_key_pem,
    )
    .unwrap();

    assert_eq!(
        get_objects_with_auth(&server, &format!("Bearer {expired}")).await,
        401,
        "expired token must be rejected"
    );

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn auth_rejects_missing_bearer_prefix(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    // Token is valid but header uses wrong scheme.
    assert_eq!(
        get_objects_with_auth(&server, &format!("Token {}", server.bootstrap_token)).await,
        401,
        "non-Bearer scheme must be rejected"
    );
    assert_eq!(
        get_objects_with_auth(&server, &server.bootstrap_token).await,
        401,
        "raw token without scheme must be rejected"
    );

    Ok(())
}

// ── RBAC permission enforcement ───────────────────────────────────────────────

/// Issue a JWT for a service account (the SA must already exist in the DB).
async fn issue_jwt(server: &RbacTestServer, namespace: &str, sa_name: &str, token_name: &str) -> String {
    let resp = server
        .api
        .get_client()
        .post(format!("{}/rbac/issue-token", server.api.get_base_url()))
        .bearer_auth(&server.bootstrap_token)
        .json(&serde_json::json!({
            "namespace": namespace,
            "service_account": sa_name,
            "token_name": token_name
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "failed to issue token");
    resp.json::<serde_json::Value>().await.unwrap()["data"]["token"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Apply a JSON object via HTTP using the given Bearer token.
async fn http_apply(server: &RbacTestServer, token: &str, body: serde_json::Value) -> serde_json::Value {
    server
        .api
        .get_client()
        .post(format!("{}/apply", server.api.get_base_url()))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// GET objects via HTTP using the given Bearer token.
async fn http_get_objects(
    server: &RbacTestServer,
    token: &str,
    filter: serde_json::Value,
) -> serde_json::Value {
    server
        .api
        .get_client()
        .post(format!("{}/get-objects", server.api.get_base_url()))
        .bearer_auth(token)
        .json(&filter)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// DELETE an object via HTTP using the given Bearer token.
async fn http_delete(server: &RbacTestServer, token: &str, body: serde_json::Value) -> serde_json::Value {
    server
        .api
        .get_client()
        .delete(format!("{}/delete-object", server.api.get_base_url()))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// Superadmin sets up:
///   - namespace `testns`
///   - serviceaccount `testns/alice`
///   - role `testns/editor` granting apply+get+delete on `container`
///   - rolebinding `testns/bind-alice` binding alice to editor
/// Returns alice's JWT.
async fn setup_alice_with_editor_role(server: &RbacTestServer) -> String {
    let sa = &server.bootstrap_token;

    // Create namespace and service account.
    http_apply(server, sa, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "testns"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "testns", "name": "alice"}
    ])).await;

    // Create a role granting full access to "container" kind.
    http_apply(server, sa, serde_json::json!({
        "api_version": "v1",
        "kind": "role",
        "namespace": "testns",
        "name": "editor",
        "rules": [{"api_version": "*", "kinds": ["container"], "verbs": ["get", "apply", "delete"]}]
    })).await;

    // Bind alice to the editor role (use 2-part FK: kind/name).
    http_apply(server, sa, serde_json::json!({
        "api_version": "v1",
        "kind": "rolebinding",
        "namespace": "testns",
        "name": "bind-alice",
        "role": "role/editor",
        "subjects": ["serviceaccount/alice"]
    })).await;

    issue_jwt(server, "testns", "alice", "alice-token").await
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_permitted_sa_can_apply_and_get(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    // Seed container schema.
    server.api.get_client()
        .post(format!("{}/apply", server.api.get_base_url()))
        .bearer_auth(&server.bootstrap_token)
        .json(&serde_json::json!({
            "api_version": "v1", "kind": "namespace", "namespace": "system", "name": "testns"
        }))
        .send().await.unwrap();

    let alice_jwt = setup_alice_with_editor_role(&server).await;

    // Alice applies a container in testns.
    let apply_resp = http_apply(&server, &alice_jwt, serde_json::json!({
        "api_version": "v2",
        "kind": "container",
        "namespace": "testns",
        "name": "box1",
        "nr": 1
    })).await;
    assert!(apply_resp["error"].is_null(), "apply should succeed: {apply_resp}");

    // Alice can read it back.
    let get_resp = http_get_objects(&server, &alice_jwt, serde_json::json!({
        "namespace": "testns",
        "kind": "container"
    })).await;
    assert!(get_resp["error"].is_null(), "get should succeed: {get_resp}");
    let items = get_resp["data"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "box1");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_unpermitted_sa_cannot_apply(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    // Create bob without any role binding.
    http_apply(&server, &server.bootstrap_token, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "testns"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "testns", "name": "bob"}
    ])).await;
    let bob_jwt = issue_jwt(&server, "testns", "bob", "bob-token").await;

    // Bob attempts to apply a container — must be forbidden.
    let resp = http_apply(&server, &bob_jwt, serde_json::json!({
        "api_version": "v2",
        "kind": "container",
        "namespace": "testns",
        "name": "box1",
        "nr": 1
    })).await;
    assert!(resp["data"].is_null(), "bob must not be able to apply: {resp}");
    assert!(!resp["error"].is_null(), "error must be set: {resp}");
    assert_eq!(resp["error"]["type"], "forbidden");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_unpermitted_sa_gets_empty_results(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    // Superadmin creates a container.
    http_apply(&server, &server.bootstrap_token, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "testns"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "testns", "name": "bob"}
    ])).await;
    http_apply(&server, &server.bootstrap_token, serde_json::json!({
        "api_version": "v2",
        "kind": "container",
        "namespace": "testns",
        "name": "secret-box",
        "nr": 42
    })).await;

    let bob_jwt = issue_jwt(&server, "testns", "bob", "bob-token").await;

    // Bob queries containers — gets empty list (no error, just nothing visible).
    let resp = http_get_objects(&server, &bob_jwt, serde_json::json!({
        "namespace": "testns",
        "kind": "container"
    })).await;
    assert!(resp["error"].is_null(), "should not error: {resp}");
    let items = get_resp_data_array(&resp);
    assert!(items.is_empty(), "bob must see no objects: {resp}");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_permitted_sa_can_delete(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let alice_jwt = setup_alice_with_editor_role(&server).await;

    // Superadmin creates the object.
    http_apply(&server, &server.bootstrap_token, serde_json::json!({
        "api_version": "v2",
        "kind": "container",
        "namespace": "testns",
        "name": "deletable",
        "nr": 1
    })).await;

    // Alice deletes it.
    let resp = http_delete(&server, &alice_jwt, serde_json::json!({
        "namespace": "testns",
        "kind": "container",
        "name": "deletable"
    })).await;
    assert!(resp["error"].is_null(), "alice must be able to delete: {resp}");

    // Confirm it is gone.
    let get_resp = http_get_objects(&server, &server.bootstrap_token, serde_json::json!({
        "namespace": "testns",
        "kind": "container"
    })).await;
    let items = get_resp_data_array(&get_resp);
    assert!(items.is_empty(), "object should be deleted");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_unpermitted_sa_cannot_delete(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    http_apply(&server, &server.bootstrap_token, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "testns"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "testns", "name": "bob"}
    ])).await;
    http_apply(&server, &server.bootstrap_token, serde_json::json!({
        "api_version": "v2",
        "kind": "container",
        "namespace": "testns",
        "name": "protected",
        "nr": 1
    })).await;

    let bob_jwt = issue_jwt(&server, "testns", "bob", "bob-token").await;

    let resp = http_delete(&server, &bob_jwt, serde_json::json!({
        "namespace": "testns",
        "kind": "container",
        "name": "protected"
    })).await;
    assert!(!resp["error"].is_null(), "delete must be forbidden: {resp}");
    assert_eq!(resp["error"]["type"], "forbidden");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_cache_invalidated_after_rolebinding_removed(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let alice_jwt = setup_alice_with_editor_role(&server).await;

    // Alice can currently apply.
    let resp = http_apply(&server, &alice_jwt, serde_json::json!({
        "api_version": "v2",
        "kind": "container",
        "namespace": "testns",
        "name": "box1",
        "nr": 1
    })).await;
    assert!(resp["error"].is_null(), "alice should be able to apply initially: {resp}");

    // Superadmin removes the role binding.
    http_delete(&server, &server.bootstrap_token, serde_json::json!({
        "namespace": "testns",
        "kind": "rolebinding",
        "name": "bind-alice"
    })).await;

    // Alice must now be denied.
    let resp = http_apply(&server, &alice_jwt, serde_json::json!({
        "api_version": "v2",
        "kind": "container",
        "namespace": "testns",
        "name": "box2",
        "nr": 2
    })).await;
    assert!(!resp["error"].is_null(), "alice must be forbidden after binding removed: {resp}");
    assert_eq!(resp["error"]["type"], "forbidden");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_superadmin_sees_all_objects(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    // Create objects in two different namespaces.
    http_apply(&server, &server.bootstrap_token, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "ns-a"},
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "ns-b"}
    ])).await;
    http_apply(&server, &server.bootstrap_token, serde_json::json!([
        {"api_version": "v2", "kind": "container", "namespace": "ns-a", "name": "box-a", "nr": 1},
        {"api_version": "v2", "kind": "container", "namespace": "ns-b", "name": "box-b", "nr": 2}
    ])).await;

    // Superadmin queries without namespace filter — must see both.
    let resp = http_get_objects(&server, &server.bootstrap_token, serde_json::json!({
        "kind": "container"
    })).await;
    assert!(resp["error"].is_null(), "superadmin get should not error: {resp}");
    let items = get_resp_data_array(&resp);
    assert_eq!(items.len(), 2, "superadmin must see objects from all namespaces");

    Ok(())
}

fn get_resp_data_array(resp: &serde_json::Value) -> Vec<&serde_json::Value> {
    resp["data"].as_array().map(|a| a.iter().collect()).unwrap_or_default()
}

// ── Additional negative GET / APPLY / FK tests ─────────────────────────────

/// Set up bob with a role that grants get+apply+delete on `container`
/// but has a name restriction to only ["allowed-parent"]. Returns bob's JWT.
async fn setup_bob_name_restricted(server: &RbacTestServer) -> String {
    let sa = &server.bootstrap_token;

    http_apply(server, sa, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "testns"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "testns", "name": "bob"}
    ])).await;

    http_apply(server, sa, serde_json::json!({
        "api_version": "v1",
        "kind": "role",
        "namespace": "testns",
        "name": "restricted",
        "rules": [{
            "api_version": "*",
            "kinds": ["container"],
            "verbs": ["get", "apply", "delete"],
            "names": ["allowed-parent"]
        }]
    })).await;

    http_apply(server, sa, serde_json::json!({
        "api_version": "v1",
        "kind": "rolebinding",
        "namespace": "testns",
        "name": "bind-bob",
        "role": "role/restricted",
        "subjects": ["serviceaccount/bob"]
    })).await;

    issue_jwt(server, "testns", "bob", "bob-token").await
}

// ── Negative GET ──────────────────────────────────────────────────────────────

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_get_restricted_to_permitted_kind_only(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let alice_jwt = setup_alice_with_editor_role(&server).await;

    // Create a serviceaccount in testns as superadmin.
    http_apply(&server, &server.bootstrap_token, serde_json::json!({
        "api_version": "v1",
        "kind": "serviceaccount",
        "namespace": "testns",
        "name": "other-sa"
    })).await;

    // Alice's role only covers "container". Querying serviceaccount should return empty.
    let resp = http_get_objects(&server, &alice_jwt, serde_json::json!({
        "namespace": "testns",
        "kind": "serviceaccount"
    })).await;
    assert!(resp["error"].is_null(), "should not error: {resp}");
    assert!(
        get_resp_data_array(&resp).is_empty(),
        "alice must not see serviceaccounts she has no role for: {resp}"
    );

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_get_name_restricted_role_filters_by_name(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let bob_jwt = setup_bob_name_restricted(&server).await;

    // Superadmin creates two containers.
    http_apply(&server, &server.bootstrap_token, serde_json::json!([
        {"api_version": "v2", "kind": "container", "namespace": "testns", "name": "allowed-parent", "nr": 1},
        {"api_version": "v2", "kind": "container", "namespace": "testns", "name": "secret-box", "nr": 2}
    ])).await;

    // Bob queries all containers — must only see "allowed-parent".
    let resp = http_get_objects(&server, &bob_jwt, serde_json::json!({
        "namespace": "testns",
        "kind": "container"
    })).await;
    assert!(resp["error"].is_null(), "should not error: {resp}");
    let items = get_resp_data_array(&resp);
    assert_eq!(items.len(), 1, "bob must see only the permitted container: {resp}");
    assert_eq!(items[0]["name"], "allowed-parent");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_get_without_kind_filter_respects_permissions(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let alice_jwt = setup_alice_with_editor_role(&server).await;

    // Superadmin creates a container and a serviceaccount in testns.
    http_apply(&server, &server.bootstrap_token, serde_json::json!([
        {"api_version": "v2", "kind": "container", "namespace": "testns", "name": "visible-box", "nr": 1},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "testns", "name": "invisible-sa"}
    ])).await;

    // Alice queries without kind filter.
    let resp = http_get_objects(&server, &alice_jwt, serde_json::json!({
        "namespace": "testns"
    })).await;
    assert!(resp["error"].is_null(), "should not error: {resp}");
    let items = get_resp_data_array(&resp);
    // Alice should see the container but NOT the serviceaccount.
    assert!(
        items.iter().any(|o| o["name"] == "visible-box"),
        "alice must see the container: {resp}"
    );
    assert!(
        !items.iter().any(|o| o["name"] == "invisible-sa"),
        "alice must not see serviceaccounts: {resp}"
    );

    Ok(())
}

// ── Negative APPLY ────────────────────────────────────────────────────────────

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_get_only_role_cannot_apply(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let sa = &server.bootstrap_token;

    http_apply(&server, sa, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "testns"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "testns", "name": "reader"}
    ])).await;

    // Role grants only `get`, not `apply`.
    http_apply(&server, sa, serde_json::json!({
        "api_version": "v1",
        "kind": "role",
        "namespace": "testns",
        "name": "readonly",
        "rules": [{"api_version": "*", "kinds": ["container"], "verbs": ["get"]}]
    })).await;
    http_apply(&server, sa, serde_json::json!({
        "api_version": "v1",
        "kind": "rolebinding",
        "namespace": "testns",
        "name": "bind-reader",
        "role": "role/readonly",
        "subjects": ["serviceaccount/reader"]
    })).await;

    let reader_jwt = issue_jwt(&server, "testns", "reader", "reader-token").await;

    let resp = http_apply(&server, &reader_jwt, serde_json::json!({
        "api_version": "v2",
        "kind": "container",
        "namespace": "testns",
        "name": "my-box",
        "nr": 1
    })).await;
    assert!(!resp["error"].is_null(), "get-only role must not apply: {resp}");
    assert_eq!(resp["error"]["type"], "forbidden");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_cannot_apply_kind_not_in_role(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    // alice has editor role for "container" only.
    let alice_jwt = setup_alice_with_editor_role(&server).await;

    // Alice tries to apply a serviceaccount (not in her role).
    let resp = http_apply(&server, &alice_jwt, serde_json::json!({
        "api_version": "v1",
        "kind": "serviceaccount",
        "namespace": "testns",
        "name": "new-sa"
    })).await;
    assert!(!resp["error"].is_null(), "applying wrong kind must be forbidden: {resp}");
    assert_eq!(resp["error"]["type"], "forbidden");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_cannot_apply_name_outside_name_restriction(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let bob_jwt = setup_bob_name_restricted(&server).await;

    // Bob tries to apply a container named "secret-box" — not in his allowed names.
    let resp = http_apply(&server, &bob_jwt, serde_json::json!({
        "api_version": "v2",
        "kind": "container",
        "namespace": "testns",
        "name": "secret-box",
        "nr": 99
    })).await;
    assert!(!resp["error"].is_null(), "applying outside name restriction must be forbidden: {resp}");
    assert_eq!(resp["error"]["type"], "forbidden");

    // Bob CAN apply the permitted name.
    let ok_resp = http_apply(&server, &bob_jwt, serde_json::json!({
        "api_version": "v2",
        "kind": "container",
        "namespace": "testns",
        "name": "allowed-parent",
        "nr": 1
    })).await;
    assert!(ok_resp["error"].is_null(), "allowed-parent should succeed: {ok_resp}");

    Ok(())
}

// ── FK access check in apply ──────────────────────────────────────────────────

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_apply_denied_when_fk_target_inaccessible(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    // Superadmin creates "secret-parent" container in testns.
    http_apply(&server, &server.bootstrap_token, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "testns"},
        {"api_version": "v2", "kind": "container", "namespace": "testns", "name": "secret-parent", "nr": 0}
    ])).await;

    let bob_jwt = setup_bob_name_restricted(&server).await;
    // Bob's role allows get/apply/delete on container, but only for names=["allowed-parent"].
    // "secret-parent" is NOT in his allowed names, so he has no Get access to it.

    // Bob tries to apply a container with parent FK pointing at "secret-parent".
    let resp = http_apply(&server, &bob_jwt, serde_json::json!({
        "api_version": "v2",
        "kind": "container",
        "namespace": "testns",
        "name": "allowed-parent",
        "nr": 1,
        "parent": "container/secret-parent"
    })).await;
    assert!(
        !resp["error"].is_null(),
        "apply must be denied when FK target is inaccessible: {resp}"
    );
    assert_eq!(resp["error"]["type"], "forbidden");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_apply_allowed_when_fk_target_is_accessible(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;

    // Superadmin creates the parent container that bob IS allowed to access.
    http_apply(&server, &server.bootstrap_token, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "testns"},
        {"api_version": "v2", "kind": "container", "namespace": "testns", "name": "allowed-parent", "nr": 0}
    ])).await;

    let bob_jwt = setup_bob_name_restricted(&server).await;

    // Bob applies a container with parent FK pointing at "allowed-parent" — must succeed.
    let resp = http_apply(&server, &bob_jwt, serde_json::json!({
        "api_version": "v2",
        "kind": "container",
        "namespace": "testns",
        "name": "allowed-parent",
        "nr": 1,
        "parent": "container/allowed-parent"
    })).await;
    assert!(
        resp["error"].is_null(),
        "apply must succeed when FK target is accessible: {resp}"
    );

    Ok(())
}

// ── rbac-test.yaml fixture ────────────────────────────────────────────────────

/// Apply all objects from examples/rbac-test.yaml in one batch (as superadmin),
/// then verify that the resulting RBAC permissions are enforced correctly.
///
/// Fixture summary:
///   - namespace "demo"
///   - alice  → admin role  (get + apply + delete on *)
///   - bob    → editor role (get + apply on *)
///   - carol  → viewer role (get on *)
#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_test_yaml_fixture_roles_are_enforced(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let sa = &server.bootstrap_token;

    // Apply the full fixture in one shot (mirrors examples/rbac-test.yaml).
    let resp = http_apply(&server, sa, serde_json::json!([
        // namespace
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "demo"},
        // service accounts
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "demo", "name": "alice"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "demo", "name": "bob"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "demo", "name": "carol"},
        // roles
        {
            "api_version": "v1", "kind": "role", "namespace": "demo", "name": "admin",
            "rules": [{"api_version": "*", "kinds": ["*"], "verbs": ["get", "apply", "delete"]}]
        },
        {
            "api_version": "v1", "kind": "role", "namespace": "demo", "name": "editor",
            "rules": [{"api_version": "*", "kinds": ["*"], "verbs": ["get", "apply"]}]
        },
        {
            "api_version": "v1", "kind": "role", "namespace": "demo", "name": "viewer",
            "rules": [{"api_version": "*", "kinds": ["*"], "verbs": ["get"]}]
        },
        // role bindings
        {
            "api_version": "v1", "kind": "rolebinding", "namespace": "demo", "name": "alice-admin",
            "role": "role/admin",
            "subjects": ["serviceaccount/alice"]
        },
        {
            "api_version": "v1", "kind": "rolebinding", "namespace": "demo", "name": "bob-editor",
            "role": "role/editor",
            "subjects": ["serviceaccount/bob"]
        },
        {
            "api_version": "v1", "kind": "rolebinding", "namespace": "demo", "name": "carol-viewer",
            "role": "role/viewer",
            "subjects": ["serviceaccount/carol"]
        }
    ])).await;
    assert!(resp["error"].is_null(), "fixture apply must succeed: {resp}");

    // Issue JWTs for each SA.
    let alice_jwt = issue_jwt(&server, "demo", "alice", "alice-token").await;
    let bob_jwt   = issue_jwt(&server, "demo", "bob",   "bob-token").await;
    let carol_jwt = issue_jwt(&server, "demo", "carol", "carol-token").await;

    // ── alice (admin): get + apply + delete ───────────────────────────────────

    let r = http_apply(&server, &alice_jwt, serde_json::json!({
        "api_version": "v2", "kind": "container", "namespace": "demo", "name": "alice-box", "nr": 1
    })).await;
    assert!(r["error"].is_null(), "alice must be able to apply: {r}");

    let r = http_get_objects(&server, &alice_jwt, serde_json::json!({
        "namespace": "demo", "kind": "container"
    })).await;
    assert!(r["error"].is_null(), "alice must be able to get: {r}");
    let items = get_resp_data_array(&r);
    assert_eq!(items.len(), 1, "alice must see her container: {r}");

    let r = http_delete(&server, &alice_jwt, serde_json::json!({
        "namespace": "demo", "kind": "container", "name": "alice-box"
    })).await;
    assert!(r["error"].is_null(), "alice must be able to delete: {r}");

    // ── bob (editor): get + apply, but NO delete ──────────────────────────────

    let r = http_apply(&server, &bob_jwt, serde_json::json!({
        "api_version": "v2", "kind": "container", "namespace": "demo", "name": "bob-box", "nr": 2
    })).await;
    assert!(r["error"].is_null(), "bob must be able to apply: {r}");

    let r = http_get_objects(&server, &bob_jwt, serde_json::json!({
        "namespace": "demo", "kind": "container"
    })).await;
    assert!(r["error"].is_null(), "bob must be able to get: {r}");
    let items = get_resp_data_array(&r);
    assert_eq!(items.len(), 1, "bob must see his container: {r}");

    let r = http_delete(&server, &bob_jwt, serde_json::json!({
        "namespace": "demo", "kind": "container", "name": "bob-box"
    })).await;
    assert!(!r["error"].is_null(), "bob must NOT be able to delete: {r}");
    assert_eq!(r["error"]["type"], "forbidden", "expected forbidden, got: {r}");

    // ── carol (viewer): get only, NO apply, NO delete ─────────────────────────

    let r = http_apply(&server, &carol_jwt, serde_json::json!({
        "api_version": "v2", "kind": "container", "namespace": "demo", "name": "carol-box", "nr": 3
    })).await;
    assert!(!r["error"].is_null(), "carol must NOT be able to apply: {r}");
    assert_eq!(r["error"]["type"], "forbidden", "expected forbidden, got: {r}");

    let r = http_get_objects(&server, &carol_jwt, serde_json::json!({
        "namespace": "demo", "kind": "container"
    })).await;
    assert!(r["error"].is_null(), "carol must be able to get: {r}");
    // bob's container exists; carol can see it
    let items = get_resp_data_array(&r);
    assert!(!items.is_empty(), "carol must see at least one container: {r}");

    let r = http_delete(&server, &carol_jwt, serde_json::json!({
        "namespace": "demo", "kind": "container", "name": "bob-box"
    })).await;
    assert!(!r["error"].is_null(), "carol must NOT be able to delete: {r}");
    assert_eq!(r["error"]["type"], "forbidden", "expected forbidden, got: {r}");

    Ok(())
}

// ── Nav-prop permission enforcement ──────────────────────────────────────────

/// A caller that can GET `rolebinding` but NOT `role` or `serviceaccount`
/// must receive `role_object` and `subjects_objects` as absent in the response
/// even when `fill_child_foreign_keys` is true.
#[sqlx::test(migrator = "MIGRATOR")]
async fn navprop_hidden_when_caller_lacks_fk_target_permission(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let sa = &server.bootstrap_token;

    // Set up namespace, service accounts, role, role binding.
    let r = http_apply(&server, sa, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "np-test"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "np-test", "name": "reader"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "np-test", "name": "target-sa"},
        // Role grants only GET on rolebinding — NOT on role or serviceaccount.
        {
            "api_version": "v1", "kind": "role", "namespace": "np-test", "name": "rb-only",
            "rules": [{"api_version": "*", "kinds": ["rolebinding"], "verbs": ["get"]}]
        },
        {
            "api_version": "v1", "kind": "role", "namespace": "np-test", "name": "target-role",
            "rules": [{"api_version": "*", "kinds": ["*"], "verbs": ["get"]}]
        },
        {
            "api_version": "v1", "kind": "rolebinding", "namespace": "np-test", "name": "bind-reader",
            "role": "role/rb-only",
            "subjects": ["serviceaccount/reader"]
        },
        {
            "api_version": "v1", "kind": "rolebinding", "namespace": "np-test", "name": "target-rb",
            "role": "role/target-role",
            "subjects": ["serviceaccount/target-sa"]
        }
    ])).await;
    assert!(r["error"].is_null(), "fixture apply must succeed: {r}");

    let reader_jwt = issue_jwt(&server, "np-test", "reader", "reader-token").await;

    // Reader fetches role bindings with fill_child_foreign_keys.
    let r = http_get_objects(&server, &reader_jwt, serde_json::json!({
        "namespace": "np-test",
        "kind": "rolebinding",
        "fill_child_foreign_keys": true
    })).await;
    assert!(r["error"].is_null(), "reader must be able to get rolebindings: {r}");

    let items = get_resp_data_array(&r);
    // reader's own binding + the target binding — both visible (rolebinding GET is allowed)
    assert!(!items.is_empty(), "must see at least one rolebinding: {r}");

    for item in &items {
        // role_object and subjects_objects must be absent/empty because the caller cannot GET role/serviceaccount.
        assert!(
            item["role_object"].is_null(),
            "role_object must be absent for restricted caller: {item}"
        );
        let subjects = &item["subjects_objects"];
        assert!(
            subjects.is_null() || subjects.as_array().map_or(false, |a| a.is_empty()),
            "subjects_objects must be absent or empty for restricted caller: {item}"
        );
    }

    Ok(())
}

/// A caller that can GET `rolebinding`, `role`, and `serviceaccount`
/// must receive populated `role_object` and `subjects_objects` nav-props.
#[sqlx::test(migrator = "MIGRATOR")]
async fn navprop_populated_when_caller_has_fk_target_permission(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let sa = &server.bootstrap_token;

    // Set up namespace, service accounts, roles, role bindings.
    let r = http_apply(&server, sa, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "np-full"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "np-full", "name": "full-reader"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "np-full", "name": "member-sa"},
        {
            "api_version": "v1", "kind": "role", "namespace": "np-full", "name": "member-role",
            "rules": [{"api_version": "*", "kinds": ["*"], "verbs": ["get"]}]
        },
        // full-reader's own role grants GET on rolebinding, role, and serviceaccount.
        {
            "api_version": "v1", "kind": "role", "namespace": "np-full", "name": "full-access",
            "rules": [{
                "api_version": "*",
                "kinds": ["rolebinding", "role", "serviceaccount"],
                "verbs": ["get"]
            }]
        },
        {
            "api_version": "v1", "kind": "rolebinding", "namespace": "np-full", "name": "bind-full-reader",
            "role": "role/full-access",
            "subjects": ["serviceaccount/full-reader"]
        },
        {
            "api_version": "v1", "kind": "rolebinding", "namespace": "np-full", "name": "target-rb",
            "role": "role/member-role",
            "subjects": ["serviceaccount/member-sa"]
        }
    ])).await;
    assert!(r["error"].is_null(), "fixture apply must succeed: {r}");

    let full_reader_jwt = issue_jwt(&server, "np-full", "full-reader", "full-reader-token").await;

    let r = http_get_objects(&server, &full_reader_jwt, serde_json::json!({
        "namespace": "np-full",
        "kind": "rolebinding",
        "name": "target-rb",
        "fill_child_foreign_keys": true
    })).await;
    assert!(r["error"].is_null(), "full-reader must be able to get rolebindings: {r}");

    let items = get_resp_data_array(&r);
    assert_eq!(items.len(), 1, "must see exactly target-rb: {r}");
    let item = &items[0];

    // role_object must be populated with the bound role.
    assert!(
        !item["role_object"].is_null(),
        "role_object must be populated for full-access caller: {item}"
    );
    assert_eq!(
        item["role_object"]["name"], "member-role",
        "role_object must point to member-role: {item}"
    );

    // subjects_objects must be a non-empty array containing member-sa.
    let subjects_objects = item["subjects_objects"].as_array();
    assert!(
        subjects_objects.is_some() && !subjects_objects.unwrap().is_empty(),
        "subjects_objects must be populated for full-access caller: {item}"
    );
    assert_eq!(
        subjects_objects.unwrap()[0]["name"], "member-sa",
        "subjects_objects[0] must be member-sa: {item}"
    );

    Ok(())
}

// ── Token revocation ──────────────────────────────────────────────────────────

/// Deleting a `ServiceAccountToken` object must immediately reject any JWT
/// that was derived from it, even if the JWT has not yet expired.
#[sqlx::test(migrator = "MIGRATOR")]
async fn deleted_token_is_rejected(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let sa = &server.bootstrap_token;

    // Create a namespace and service account.
    let r = http_apply(&server, sa, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "revoke-test"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "revoke-test", "name": "alice"}
    ])).await;
    assert!(r["error"].is_null(), "fixture apply must succeed: {r}");

    // Issue a token for alice — this goes through /rbac/issue-token and adds the
    // UUID to the valid-token cache.
    let alice_jwt = issue_jwt(&server, "revoke-test", "alice", "alice-token").await;

    // The JWT must work before revocation.
    let r = http_get_objects(&server, &alice_jwt, serde_json::json!({
        "namespace": "revoke-test", "kind": "serviceaccount"
    })).await;
    // alice has no role bindings so she gets an empty list — but NOT a 401.
    assert!(r["error"].is_null(), "alice JWT must be accepted before revocation: {r}");

    // Delete the ServiceAccountToken object — this should revoke the JWT.
    let r = http_delete(&server, sa, serde_json::json!({
        "namespace": "revoke-test", "kind": "serviceaccounttoken", "name": "alice-token"
    })).await;
    assert!(r["error"].is_null(), "superadmin must be able to delete the token: {r}");

    // The same JWT must now be rejected with 401.
    let resp = server
        .api
        .get_client()
        .post(format!("{}/get-objects", server.api.get_base_url()))
        .bearer_auth(&alice_jwt)
        .json(&serde_json::json!({"namespace": "revoke-test", "kind": "serviceaccount"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "revoked JWT must be rejected with 401");

    Ok(())
}

// ── Problem 2: namespace-scoped grants must not cross namespace boundaries ────

/// A caller whose RoleBinding is in `ns-a` must be denied apply/delete in `ns-b`,
/// even if the role grants the same kind.
#[sqlx::test(migrator = "MIGRATOR")]
async fn namespace_scoped_grant_does_not_cross_namespace(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let sa = &server.bootstrap_token;

    // Two namespaces, one SA in ns-a, one role granting apply+delete on container in ns-a.
    let r = http_apply(&server, sa, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "ns-a"},
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "ns-b"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "ns-a", "name": "alice"},
        {
            "api_version": "v1", "kind": "role", "namespace": "ns-a", "name": "editor",
            "rules": [{"api_version": "*", "kinds": ["container"], "verbs": ["get", "apply", "delete"]}]
        },
        {
            "api_version": "v1", "kind": "rolebinding", "namespace": "ns-a", "name": "bind-alice",
            "role": "role/editor",
            "subjects": ["serviceaccount/alice"]
        }
    ])).await;
    assert!(r["error"].is_null(), "fixture apply must succeed: {r}");

    let alice_jwt = issue_jwt(&server, "ns-a", "alice", "alice-tok").await;

    // Alice can apply in her own namespace.
    let r = http_apply(&server, &alice_jwt, serde_json::json!({
        "api_version": "v2", "kind": "container", "namespace": "ns-a", "name": "own", "nr": 1
    })).await;
    assert!(r["error"].is_null(), "alice must apply in ns-a: {r}");

    // Alice must be denied apply in the foreign namespace.
    let r = http_apply(&server, &alice_jwt, serde_json::json!({
        "api_version": "v2", "kind": "container", "namespace": "ns-b", "name": "foreign", "nr": 2
    })).await;
    assert!(!r["error"].is_null(), "alice must NOT apply in ns-b: {r}");
    assert_eq!(r["error"]["type"], "forbidden", "expected forbidden: {r}");

    // Superadmin seeds a container in ns-b for the delete test.
    let r = http_apply(&server, sa, serde_json::json!({
        "api_version": "v2", "kind": "container", "namespace": "ns-b", "name": "victim", "nr": 3
    })).await;
    assert!(r["error"].is_null(), "superadmin seeding ns-b container: {r}");

    // Alice must be denied delete in the foreign namespace.
    let r = http_delete(&server, &alice_jwt, serde_json::json!({
        "namespace": "ns-b", "kind": "container", "name": "victim"
    })).await;
    assert!(!r["error"].is_null(), "alice must NOT delete in ns-b: {r}");
    assert_eq!(r["error"]["type"], "forbidden", "expected forbidden: {r}");

    Ok(())
}

// ── Problem 3: namespace restriction must apply to nav-prop embedded objects ──

/// Embedding a `Namespace` object in a nav-prop field must not bypass the
/// restriction that namespaces may only be created in the `system` namespace.
#[sqlx::test(migrator = "MIGRATOR")]
async fn navprop_embedded_namespace_outside_system_is_rejected(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let sa = &server.bootstrap_token;

    // Seed a namespace and a container object so the apply has something to reference.
    let r = http_apply(&server, sa, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "embed-test"},
        {"api_version": "v2", "kind": "container", "namespace": "embed-test", "name": "host", "nr": 1}
    ])).await;
    assert!(r["error"].is_null(), "fixture apply must succeed: {r}");

    // Attempt to embed a Namespace object targeting a non-system namespace inside
    // a nav-prop field of an otherwise valid container object.
    let r = http_apply(&server, sa, serde_json::json!({
        "api_version": "v2", "kind": "container", "namespace": "embed-test", "name": "carrier", "nr": 2,
        "evil_object": {
            "api_version": "v1", "kind": "namespace",
            "namespace": "embed-test", "name": "injected"
        }
    })).await;
    assert!(!r["error"].is_null(), "embedded namespace outside system must be rejected: {r}");

    // The namespace object must not have been created.
    let r = http_get_objects(&server, sa, serde_json::json!({
        "kind": "namespace", "name": "injected"
    })).await;
    assert!(r["error"].is_null(), "get must not error: {r}");
    let items = get_resp_data_array(&r);
    assert!(items.is_empty(), "injected namespace must not exist: {r}");

    Ok(())
}

// ── Problem 4: privilege escalation prevention ────────────────────────────────

/// A caller with a limited Role cannot create a Role that grants more permissions
/// than they themselves hold.
#[sqlx::test(migrator = "MIGRATOR")]
async fn caller_cannot_create_role_with_excess_permissions(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let sa = &server.bootstrap_token;

    // Set up alice with a role that only grants `get` on `container`.
    http_apply(&server, sa, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "myns"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "myns", "name": "alice"}
    ])).await;

    // Alice's role: get on container AND apply/get on role (so she can create roles).
    let r = http_apply(&server, sa, serde_json::json!({
        "api_version": "v1", "kind": "role", "namespace": "myns", "name": "getter",
        "rules": [
            {"api_version": "*", "kinds": ["container"], "verbs": ["get"]},
            {"api_version": "*", "kinds": ["role"], "verbs": ["apply", "get"]}
        ]
    })).await;
    assert!(r["error"].is_null(), "superadmin seeding getter role: {r}");

    let r = http_apply(&server, sa, serde_json::json!({
        "api_version": "v1", "kind": "rolebinding", "namespace": "myns", "name": "bind-alice",
        "role": "role/getter",
        "subjects": ["serviceaccount/alice"]
    })).await;
    assert!(r["error"].is_null(), "superadmin binding alice: {r}");

    let alice_jwt = issue_jwt(&server, "myns", "alice", "alice-token").await;

    // Alice tries to create a role with `apply` on container (which she doesn't have).
    let r = http_apply(&server, &alice_jwt, serde_json::json!({
        "api_version": "v1", "kind": "role", "namespace": "myns", "name": "escalated",
        "rules": [{"api_version": "*", "kinds": ["container"], "verbs": ["apply"]}]
    })).await;
    assert!(!r["error"].is_null(), "alice must not create a role with excess permissions: {r}");
    assert_eq!(r["error"]["type"], "forbidden", "expected forbidden: {r}");

    // Alice can create a role with only `get` on container (which she does have).
    let r = http_apply(&server, &alice_jwt, serde_json::json!({
        "api_version": "v1", "kind": "role", "namespace": "myns", "name": "ok-role",
        "rules": [{"api_version": "*", "kinds": ["container"], "verbs": ["get"]}]
    })).await;
    assert!(r["error"].is_null(), "alice should be able to create a role within her own permissions: {r}");

    Ok(())
}

/// A caller cannot create a RoleBinding that references a Role granting more
/// permissions than the caller holds, even if they can `apply` the binding.
#[sqlx::test(migrator = "MIGRATOR")]
async fn caller_cannot_bind_role_with_excess_permissions(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let sa = &server.bootstrap_token;

    // Create alice with `apply` on `rolebinding` but only `get` on `container`.
    // The powerful role she'll try to bind grants `apply` on `container`.
    http_apply(&server, sa, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "bindns"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "bindns", "name": "alice"}
    ])).await;

    // A powerful role that alice does not possess herself.
    let r = http_apply(&server, sa, serde_json::json!({
        "api_version": "v1", "kind": "role", "namespace": "bindns", "name": "powerful",
        "rules": [{"api_version": "*", "kinds": ["container"], "verbs": ["apply", "delete"]}]
    })).await;
    assert!(r["error"].is_null(), "superadmin creates powerful role: {r}");

    // Alice's own role: she can apply rolebindings and get containers, but not apply/delete them.
    let r = http_apply(&server, sa, serde_json::json!({
        "api_version": "v1", "kind": "role", "namespace": "bindns", "name": "alice-role",
        "rules": [
            {"api_version": "*", "kinds": ["rolebinding"], "verbs": ["apply", "get"]},
            {"api_version": "*", "kinds": ["container"], "verbs": ["get"]}
        ]
    })).await;
    assert!(r["error"].is_null(), "superadmin creates alice-role: {r}");

    let r = http_apply(&server, sa, serde_json::json!({
        "api_version": "v1", "kind": "rolebinding", "namespace": "bindns", "name": "bind-alice",
        "role": "role/alice-role",
        "subjects": ["serviceaccount/alice"]
    })).await;
    assert!(r["error"].is_null(), "superadmin binds alice: {r}");

    let alice_jwt = issue_jwt(&server, "bindns", "alice", "alice-token").await;

    // Create a target SA that alice would bind to the powerful role.
    http_apply(&server, sa, serde_json::json!({
        "api_version": "v1", "kind": "serviceaccount", "namespace": "bindns", "name": "victim"
    })).await;

    // Alice tries to bind `victim` to the powerful role — this escalates privileges.
    let r = http_apply(&server, &alice_jwt, serde_json::json!({
        "api_version": "v1", "kind": "rolebinding", "namespace": "bindns", "name": "escalation-binding",
        "role": "role/powerful",
        "subjects": ["serviceaccount/victim"]
    })).await;
    assert!(!r["error"].is_null(), "alice must not bind a role exceeding her own permissions: {r}");
    assert_eq!(r["error"]["type"], "forbidden", "expected forbidden: {r}");

    Ok(())
}

/// A caller with only namespace-scoped grants cannot create a GlobalRole (which
/// would confer cross-namespace powers they do not hold).
#[sqlx::test(migrator = "MIGRATOR")]
async fn namespaced_caller_cannot_create_global_role(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let sa = &server.bootstrap_token;

    http_apply(&server, sa, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "testns"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "testns", "name": "alice"}
    ])).await;

    // Alice has full apply/get/delete on container via a *namespace-scoped* role.
    let r = http_apply(&server, sa, serde_json::json!({
        "api_version": "v1", "kind": "role", "namespace": "testns", "name": "full",
        "rules": [{"api_version": "*", "kinds": ["container"], "verbs": ["get", "apply", "delete"]}]
    })).await;
    assert!(r["error"].is_null(), "superadmin creates full role: {r}");

    // Also give alice permission to apply globalrole objects.
    let r = http_apply(&server, sa, serde_json::json!({
        "api_version": "v1", "kind": "role", "namespace": "testns", "name": "rbac-role",
        "rules": [{"api_version": "*", "kinds": ["globalrole"], "verbs": ["apply", "get"]}]
    })).await;
    assert!(r["error"].is_null(), "superadmin creates rbac-role: {r}");

    let r = http_apply(&server, sa, serde_json::json!([
        {
            "api_version": "v1", "kind": "rolebinding", "namespace": "testns", "name": "bind-alice",
            "role": "role/full", "subjects": ["serviceaccount/alice"]
        },
        {
            "api_version": "v1", "kind": "rolebinding", "namespace": "testns", "name": "bind-alice-rbac",
            "role": "role/rbac-role", "subjects": ["serviceaccount/alice"]
        }
    ])).await;
    assert!(r["error"].is_null(), "superadmin binds alice: {r}");

    let alice_jwt = issue_jwt(&server, "testns", "alice", "alice-token").await;

    // Alice tries to create a GlobalRole granting container access across all namespaces.
    // She holds the permission namespace-scoped only, so this must be rejected.
    let r = http_apply(&server, &alice_jwt, serde_json::json!({
        "api_version": "v1", "kind": "globalrole", "namespace": "testns", "name": "evil-global",
        "rules": [{"api_version": "*", "kinds": ["container"], "verbs": ["get", "apply", "delete"]}]
    })).await;
    assert!(!r["error"].is_null(), "namespaced caller must not create a GlobalRole: {r}");
    assert_eq!(r["error"]["type"], "forbidden", "expected forbidden: {r}");

    Ok(())
}

// ── Problem 6: SA deletion must evict its permission cache entry ──────────────

/// Deleting a `ServiceAccount` must clear its cached permissions so that a
/// new SA created with the same (namespace, name) does not inherit the old
/// SA's grants.
///
/// Test scenario: alice is granted editor access, her SA is fully deleted (tokens
/// → rolebinding → SA), then a new alice SA is created with no bindings and
/// issued a fresh token. The new alice must have no permissions.
#[sqlx::test(migrator = "MIGRATOR")]
async fn deleted_sa_does_not_leak_permissions_to_recreated_sa(pool: PgPool) -> sqlx::Result<()> {
    let server = spawn_rbac_server(pool).await;
    let sa = &server.bootstrap_token;

    // Setup: namespace, old alice with editor access.
    http_apply(&server, sa, serde_json::json!([
        {"api_version": "v1", "kind": "namespace", "namespace": "system", "name": "recycled-ns"},
        {"api_version": "v1", "kind": "serviceaccount", "namespace": "recycled-ns", "name": "alice"}
    ])).await;

    http_apply(&server, sa, serde_json::json!({
        "api_version": "v1", "kind": "role", "namespace": "recycled-ns", "name": "editor",
        "rules": [{"api_version": "*", "kinds": ["container"], "verbs": ["get", "apply", "delete"]}]
    })).await;

    http_apply(&server, sa, serde_json::json!({
        "api_version": "v1", "kind": "rolebinding", "namespace": "recycled-ns", "name": "bind-alice",
        "role": "role/editor", "subjects": ["serviceaccount/alice"]
    })).await;

    let alice_jwt = issue_jwt(&server, "recycled-ns", "alice", "alice-token").await;

    // Warm the cache: alice makes a request so her permissions are loaded.
    let r = http_apply(&server, &alice_jwt, serde_json::json!({
        "api_version": "v2", "kind": "container", "namespace": "recycled-ns", "name": "box1", "nr": 1
    })).await;
    assert!(r["error"].is_null(), "old alice must be able to apply: {r}");

    // Tear down alice: token → rolebinding → SA (FK constraints require this order).
    http_delete(&server, sa, serde_json::json!({
        "namespace": "recycled-ns", "kind": "serviceaccounttoken", "name": "alice-token"
    })).await;
    http_delete(&server, sa, serde_json::json!({
        "namespace": "recycled-ns", "kind": "rolebinding", "name": "bind-alice"
    })).await;
    http_delete(&server, sa, serde_json::json!({
        "namespace": "recycled-ns", "kind": "serviceaccount", "name": "alice"
    })).await;

    // Re-create alice with NO rolebinding (no permissions).
    http_apply(&server, sa, serde_json::json!({
        "api_version": "v1", "kind": "serviceaccount", "namespace": "recycled-ns", "name": "alice"
    })).await;

    let new_alice_jwt = issue_jwt(&server, "recycled-ns", "alice", "alice-token2").await;

    // New alice must not inherit old alice's cached permissions.
    let r = http_apply(&server, &new_alice_jwt, serde_json::json!({
        "api_version": "v2", "kind": "container", "namespace": "recycled-ns", "name": "box2", "nr": 2
    })).await;
    assert!(!r["error"].is_null(), "new alice must not inherit old alice's permissions: {r}");
    assert_eq!(r["error"]["type"], "forbidden", "expected forbidden: {r}");

    Ok(())
}

// ── Namespace lifecycle ───────────────────────────────────────────────────────

/// Applying an object to a namespace that has not been created must be rejected.
#[sqlx::test(migrator = "MIGRATOR")]
async fn apply_to_nonexistent_namespace_is_rejected(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    let result = api
        .apply_str(serde_json::to_string(&serde_json::json!({
            "api_version": "v1",
            "kind": "container",
            "namespace": "ghost-ns",
            "name": "box-1",
            "nr": 1,
        }))
        .unwrap())
        .await;

    assert!(result.is_err(), "apply to non-existent namespace must fail");
    let dawnstore_client_lib::DawnstoreApiError::ServerError(err) = result.unwrap_err() else {
        panic!("expected ServerError");
    };
    assert!(
        matches!(err, dawnstore_lib::DawnStoreApiError::ValidationError { .. }),
        "expected ValidationError, got {err:?}",
    );

    Ok(())
}

/// After creating a namespace, objects can be applied into it.
#[sqlx::test(migrator = "MIGRATOR")]
async fn apply_to_existing_namespace_succeeds(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    // Create the namespace first.
    api.apply_str(serde_json::to_string(&serde_json::json!({
        "api_version": "v1",
        "kind": "namespace",
        "namespace": "system",
        "name": "myns",
    }))
    .unwrap())
    .await
    .unwrap();

    // Now apply a container into it — must succeed.
    let result = api
        .apply_str(serde_json::to_string(&serde_json::json!({
            "api_version": "v1",
            "kind": "container",
            "namespace": "myns",
            "name": "box-1",
            "nr": 42,
        }))
        .unwrap())
        .await
        .unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].namespace, "myns");
    assert_eq!(result[0].name, "box-1");

    Ok(())
}

/// Namespace objects can only be created in the `system` namespace.
#[sqlx::test(migrator = "MIGRATOR")]
async fn namespace_object_must_live_in_system_namespace(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    let result = api
        .apply_str(serde_json::to_string(&serde_json::json!({
            "api_version": "v1",
            "kind": "namespace",
            "namespace": "default",   // wrong — must be "system"
            "name": "bad-ns",
        }))
        .unwrap())
        .await;

    assert!(result.is_err(), "namespace in wrong namespace must be rejected");
    let dawnstore_client_lib::DawnstoreApiError::ServerError(err) = result.unwrap_err() else {
        panic!("expected ServerError");
    };
    assert!(
        matches!(err, dawnstore_lib::DawnStoreApiError::NamespaceRestriction { .. }),
        "expected NamespaceRestriction, got {err:?}",
    );

    Ok(())
}

/// Deleting a namespace cascades to all objects inside it.
#[sqlx::test(migrator = "MIGRATOR")]
async fn delete_namespace_cascades_objects(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    // Create namespace.
    api.apply_str(serde_json::to_string(&serde_json::json!({
        "api_version": "v1", "kind": "namespace", "namespace": "system", "name": "myns",
    }))
    .unwrap())
    .await
    .unwrap();

    // Create two containers in it.
    for name in ["box-a", "box-b"] {
        api.apply_str(serde_json::to_string(&serde_json::json!({
            "api_version": "v1", "kind": "container",
            "namespace": "myns", "name": name, "nr": 1,
        }))
        .unwrap())
        .await
        .unwrap();
    }

    // Confirm they exist.
    let before = api
        .get_objects(&GetObjectsFilter { namespace: Some("myns".into()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(before.len(), 2, "expected 2 containers before delete");

    // Delete the namespace.
    api.delete_object(&DeleteObject {
        namespace: Some("system".into()),
        kind: "namespace".into(),
        name: "myns".into(),
    })
    .await
    .unwrap();

    // All containers must be gone.
    let after = api
        .get_objects(&GetObjectsFilter { namespace: Some("myns".into()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(after.len(), 0, "all objects in deleted namespace must be removed");

    // The namespace object itself must also be gone.
    let ns_objects = api
        .get_objects(&GetObjectsFilter {
            namespace: Some("system".into()),
            kind: Some("namespace".into()),
            name: Some("myns".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(ns_objects.len(), 0, "namespace object itself must be deleted");

    Ok(())
}

/// Deleting a namespace is blocked when objects in OTHER namespaces hold FK
/// references into it.
#[sqlx::test(migrator = "MIGRATOR")]
async fn delete_namespace_blocked_by_cross_namespace_references(pool: PgPool) -> sqlx::Result<()> {
    let api = spawn_server(pool).await;

    // Create two namespaces.
    for ns in ["ns-a", "ns-b"] {
        api.apply_str(serde_json::to_string(&serde_json::json!({
            "api_version": "v1", "kind": "namespace", "namespace": "system", "name": ns,
        }))
        .unwrap())
        .await
        .unwrap();
    }

    // Create a container in ns-a.
    api.apply_str(serde_json::to_string(&serde_json::json!({
        "api_version": "v1", "kind": "container",
        "namespace": "ns-a", "name": "target", "nr": 1,
    }))
    .unwrap())
    .await
    .unwrap();

    // Create a container in ns-b that references the one in ns-a via a
    // cross-namespace FK (3-segment format: namespace/kind/name).
    api.apply_str(serde_json::to_string(&serde_json::json!({
        "api_version": "v1", "kind": "container",
        "namespace": "ns-b", "name": "referrer", "nr": 2,
        "parent": "ns-a/container/target",
    }))
    .unwrap())
    .await
    .unwrap();

    // Deleting ns-a must be rejected because ns-b still references its object.
    let result = api
        .delete_object(&DeleteObject {
            namespace: Some("system".into()),
            kind: "namespace".into(),
            name: "ns-a".into(),
        })
        .await;

    assert!(result.is_err(), "delete of ns-a must be blocked by cross-namespace reference");
    let dawnstore_client_lib::DawnstoreApiError::ServerError(err) = result.unwrap_err() else {
        panic!("expected ServerError");
    };
    assert!(
        matches!(err, dawnstore_lib::DawnStoreApiError::ValidationError { .. }),
        "expected ValidationError, got {err:?}",
    );

    // ns-a objects must still exist.
    let ns_a_objects = api
        .get_objects(&GetObjectsFilter { namespace: Some("ns-a".into()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(ns_a_objects.len(), 1, "ns-a objects must not be deleted");

    // After deleting ns-b (removing the cross-namespace reference), ns-a can be deleted.
    api.delete_object(&DeleteObject {
        namespace: Some("system".into()),
        kind: "namespace".into(),
        name: "ns-b".into(),
    })
    .await
    .unwrap();

    api.delete_object(&DeleteObject {
        namespace: Some("system".into()),
        kind: "namespace".into(),
        name: "ns-a".into(),
    })
    .await
    .unwrap();

    let ns_a_after = api
        .get_objects(&GetObjectsFilter { namespace: Some("ns-a".into()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(ns_a_after.len(), 0, "ns-a objects must be gone after cascade delete");

    Ok(())
}
