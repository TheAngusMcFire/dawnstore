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
    assert_eq!(objects.len(), 1, "should find the system namespace");
    assert_eq!(objects[0]["name"], "system");
    assert_eq!(objects[0]["namespace"], "system");

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
    assert_eq!(objects2.len(), 1);
    assert_eq!(objects2[0]["name"], "system");

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

// ── RBAC ─────────────────────────────────────────────────────────────────────
//
// Tests for rbac::init (seeding), rbac::bootstrap, and /rbac/issue-token.
// Seeding / bootstrap tests run directly against the backend; HTTP-layer tests
// use a full server with JWT middleware.

use dawnstore_core::abstractions::DawnstoreBackend;
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

    let bootstrap_token =
        dawnstore_core::rbac::bootstrap(&backend, &keypair.private_key_pem)
            .await
            .unwrap()
            .expect("first startup must return a bootstrap token");

    let backend = Arc::new(backend);
    let dawnstore_routes = get_dawnstore_default_routes(Arc::clone(&backend));
    let rbac_routes = dawnstore_core::rbac::get_rbac_routes(
        Arc::clone(&backend),
        keypair.private_key_pem.clone(),
    );
    let app = dawnstore_core::rbac::with_jwt_auth(
        Router::new().merge(dawnstore_routes).merge(rbac_routes),
        keypair.public_key_pem.clone(),
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
    use dawnstore_core::rbac::authz_service::is_superadmin;
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

    let objects = DawnstoreBackend::get(&backend, &GetObjectsFilter {
            namespace: Some("system".into()),
            kind: Some("namespace".into()),
            name: Some("system".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].name, "system");
    assert_eq!(objects[0].namespace, "system");
    assert_eq!(objects[0].kind, "namespace");

    Ok(())
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rbac_init_seeds_superadmin(pool: PgPool) -> sqlx::Result<()> {
    let backend = PostgresBackend::new(pool);
    dawnstore_core::rbac::init(&backend).await.unwrap();

    let objects = DawnstoreBackend::get(&backend, &GetObjectsFilter {
            namespace: Some("system".into()),
            kind: Some("serviceaccount".into()),
            name: Some("superadmin".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].name, "superadmin");
    assert_eq!(objects[0].namespace, "system");

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
    let jwt = body["token"].as_str().expect("response must have a token field");
    let token_id_str = body["token_id"].as_str().expect("response must have a token_id field");
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

    // Create a regular service account in a separate namespace.
    let apply_resp = server
        .api
        .get_client()
        .post(format!("{}/apply", server.api.get_base_url()))
        .bearer_auth(&server.bootstrap_token)
        .header("content-type", "application/json")
        .body(
            serde_json::json!({
                "api_version": "v1",
                "kind": "serviceaccount",
                "namespace": "test-ns",
                "name": "regular"
            })
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
    let regular_jwt = regular_jwt["token"].as_str().unwrap().to_string();

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
