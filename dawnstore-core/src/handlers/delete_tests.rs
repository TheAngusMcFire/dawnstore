#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::future::Future;

    use chrono::Utc;
    use dawnstore_lib::{DeleteObject, ObjectAny, ReturnObject};
    use serde_json::{json, Value};
    use uuid::Uuid;

    use crate::abstractions::{
        BackendGetObjectsFilter, NewDawnStoreBackend, ObjectRelation, RawForeignKeyConstraint,
        RawSchema,
    };
    use crate::cache::DawnstoreCache;
    use crate::error::DawnStoreError;
    use crate::handlers::delete::delete;
    use crate::rbac::cache::{EffectivePermissions, GrantedScope, Verb};
    use crate::rbac::helpers::object_string_id;
    use crate::rbac::middleware::Claims;

    // ── MockBackend ───────────────────────────────────────────────────────────

    struct MockBackend {
        schemas: Vec<RawSchema>,
        objects: std::sync::Mutex<HashMap<String, Value>>,
    }

    impl MockBackend {
        fn new() -> Self {
            Self { schemas: vec![], objects: std::sync::Mutex::new(HashMap::new()) }
        }

        fn with_schema(mut self, schema: RawSchema) -> Self {
            self.schemas.push(schema);
            self
        }

        fn with_object(self, obj: ReturnObject<Value>) -> Self {
            let key = object_string_id(&obj.namespace, &obj.kind, &obj.name);
            let value = serde_json::to_value(&obj).unwrap();
            self.objects.lock().unwrap().insert(key, value);
            self
        }

        fn contains(&self, namespace: &str, kind: &str, name: &str) -> bool {
            let key = object_string_id(namespace, kind, name);
            self.objects.lock().unwrap().contains_key(&key)
        }
    }

    impl NewDawnStoreBackend for MockBackend {
        fn load_all_schemas(
            &self,
        ) -> impl Future<Output = Result<Vec<RawSchema>, DawnStoreError>> + Send {
            let schemas = self.schemas.clone();
            async move { Ok(schemas) }
        }

        fn load_all_foreign_key_constraints(
            &self,
        ) -> impl Future<Output = Result<Vec<RawForeignKeyConstraint>, DawnStoreError>> + Send
        {
            async move { Ok(vec![]) }
        }

        fn get_objects(
            &self,
            _filter: &BackendGetObjectsFilter,
        ) -> impl Future<Output = Result<Vec<ReturnObject<Value>>, DawnStoreError>> + Send {
            async move { Ok(vec![]) }
        }

        fn get_object(
            &self,
            namespace: &str,
            kind: &str,
            name: &str,
        ) -> impl Future<Output = Result<Option<ReturnObject<Value>>, DawnStoreError>> + Send
        {
            let key = object_string_id(namespace, kind, name);
            let result = {
                let objects = self.objects.lock().unwrap();
                objects
                    .get(&key)
                    .map(|v| serde_json::from_value::<ReturnObject<Value>>(v.clone()).unwrap())
            };
            async move { Ok(result) }
        }

        fn upsert_objects(
            &self,
            _objects: Vec<ObjectAny>,
            _relations: Vec<ObjectRelation>,
        ) -> impl Future<Output = Result<Vec<ReturnObject<Value>>, DawnStoreError>> + Send {
            async move { Ok(vec![]) }
        }

        fn delete_object(
            &self,
            namespace: &str,
            kind: &str,
            name: &str,
        ) -> impl Future<Output = Result<(), DawnStoreError>> + Send {
            let key = object_string_id(namespace, kind, name);
            self.objects.lock().unwrap().remove(&key);
            async move { Ok(()) }
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn make_claims(namespace: &str, sa_name: &str) -> Claims {
        Claims {
            sub: sa_name.to_string(),
            namespace: namespace.to_string(),
            token_name: "test-token".to_string(),
            token_id: Uuid::new_v4(),
            exp: u64::MAX,
        }
    }

    fn permissive_schema(api_version: &str, kind: &str) -> RawSchema {
        RawSchema {
            api_version: api_version.to_string(),
            kind: kind.to_string(),
            aliases: vec![],
            json_schema: r#"{"type": "object"}"#.to_string(),
        }
    }

    fn schema_with_aliases(api_version: &str, kind: &str, aliases: &[&str]) -> RawSchema {
        RawSchema {
            api_version: api_version.to_string(),
            kind: kind.to_string(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            json_schema: r#"{"type": "object"}"#.to_string(),
        }
    }

    fn make_return_object(
        namespace: &str,
        api_version: &str,
        kind: &str,
        name: &str,
    ) -> ReturnObject<Value> {
        ReturnObject {
            id: Uuid::new_v4(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            annotations: None,
            labels: None,
            namespace: namespace.to_string(),
            api_version: api_version.to_string(),
            kind: kind.to_string(),
            name: name.to_string(),
            spec: json!({}),
        }
    }

    fn wildcard_delete() -> GrantedScope {
        GrantedScope {
            api_version: "*".to_string(),
            kinds: vec!["*".to_string()],
            verbs: [Verb::Delete].into_iter().collect(),
            names: None,
        }
    }

    fn delete_request(namespace: &str, kind: &str, name: &str) -> DeleteObject {
        DeleteObject {
            namespace: Some(namespace.to_string()),
            kind: kind.to_string(),
            name: name.to_string(),
        }
    }

    async fn init_cache(backend: &MockBackend) -> DawnstoreCache {
        DawnstoreCache::init(backend).await.unwrap()
    }

    // ── Positive tests ────────────────────────────────────────────────────────

    /// Unauthenticated (superadmin) path deletes the object successfully.
    #[tokio::test]
    async fn test_unauthenticated_delete_succeeds() {
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_object(make_return_object("default", "v1", "Car", "car1"));
        let cache = init_cache(&backend).await;

        assert!(backend.contains("default", "Car", "car1"));
        let result = delete(&backend, &cache, None, delete_request("default", "Car", "car1")).await;
        assert!(result.is_ok());
        assert!(!backend.contains("default", "Car", "car1"));
    }

    /// Authenticated caller with Delete permission deletes the object.
    #[tokio::test]
    async fn test_authenticated_delete_with_permission_succeeds() {
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_object(make_return_object("default", "v1", "Car", "car1"));
        let cache = init_cache(&backend).await;

        let caller = make_claims("default", "svc-a");
        cache.insert_permissions(
            "default",
            "svc-a",
            EffectivePermissions { namespaced: vec![wildcard_delete()], global: vec![] },
            vec![],
        );

        let result =
            delete(&backend, &cache, Some(&caller), delete_request("default", "Car", "car1"))
                .await;
        assert!(result.is_ok());
        assert!(!backend.contains("default", "Car", "car1"));
    }

    /// Deleting a non-existent object is idempotent — returns Ok.
    #[tokio::test]
    async fn test_delete_nonexistent_object_is_ok() {
        let backend = MockBackend::new().with_schema(permissive_schema("v1", "Car"));
        let cache = init_cache(&backend).await;

        let result =
            delete(&backend, &cache, None, delete_request("default", "Car", "ghost")).await;
        assert!(result.is_ok());
    }

    /// Kind alias is resolved before deletion.
    #[tokio::test]
    async fn test_kind_alias_is_resolved_before_delete() {
        let backend = MockBackend::new()
            .with_schema(schema_with_aliases("v1", "Car", &["cars"]))
            .with_object(make_return_object("default", "v1", "Car", "car1"));
        let cache = init_cache(&backend).await;

        // Use alias "cars" in the request.
        let result = delete(
            &backend,
            &cache,
            None,
            DeleteObject { namespace: Some("default".to_string()), kind: "cars".to_string(), name: "car1".to_string() },
        )
        .await;
        assert!(result.is_ok());
        assert!(!backend.contains("default", "Car", "car1"));
    }

    /// Superadmin SA bypasses permission checks even when `caller` is Some.
    #[tokio::test]
    async fn test_superadmin_caller_bypasses_permission_check() {
        use crate::rbac::constants::{SA_SUPERADMIN, SYSTEM_NAMESPACE};

        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_object(make_return_object("default", "v1", "Car", "car1"));
        let cache = init_cache(&backend).await;

        let caller = Claims {
            sub: SA_SUPERADMIN.to_string(),
            namespace: SYSTEM_NAMESPACE.to_string(),
            token_name: "bootstrap".to_string(),
            token_id: Uuid::new_v4(),
            exp: u64::MAX,
        };
        // No permissions injected — superadmin bypasses all checks.

        let result =
            delete(&backend, &cache, Some(&caller), delete_request("default", "Car", "car1"))
                .await;
        assert!(result.is_ok());
    }

    /// Deleting an RBAC resource evicts permission-cache entries derived from it.
    #[tokio::test]
    async fn test_delete_rbac_resource_invalidates_permission_cache() {
        use crate::rbac::constants::{API_VERSION_V1, KIND_ROLE_BINDING};

        let backend = MockBackend::new()
            .with_schema(permissive_schema(API_VERSION_V1, KIND_ROLE_BINDING));
        let cache = init_cache(&backend).await;

        // Inject permissions for svc-a, recording that they came from a rolebinding.
        let rb_sid = object_string_id("default", KIND_ROLE_BINDING, "my-rb");
        cache.insert_permissions(
            "default",
            "svc-a",
            EffectivePermissions { namespaced: vec![wildcard_delete()], global: vec![] },
            vec![rb_sid],
        );
        assert!(cache.get_permissions("default", "svc-a").is_some());

        // Delete the rolebinding — should evict svc-a's permissions.
        let result = delete(
            &backend,
            &cache,
            None,
            delete_request("default", KIND_ROLE_BINDING, "my-rb"),
        )
        .await;
        assert!(result.is_ok());
        assert!(
            cache.get_permissions("default", "svc-a").is_none(),
            "permissions should be evicted after RBAC resource deletion"
        );
    }

    // ── Negative tests ────────────────────────────────────────────────────────

    /// Authenticated caller without Delete permission receives Forbidden.
    #[tokio::test]
    async fn test_no_delete_permission_returns_forbidden() {
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_object(make_return_object("default", "v1", "Car", "car1"));
        let cache = init_cache(&backend).await;

        let caller = make_claims("default", "svc-a");
        // No permissions injected.

        let result =
            delete(&backend, &cache, Some(&caller), delete_request("default", "Car", "car1"))
                .await;
        assert!(matches!(result, Err(DawnStoreError::Forbidden)));
        // Object must still exist.
        assert!(backend.contains("default", "Car", "car1"));
    }

    /// Deleting an unregistered kind returns UnknownResourceKind.
    #[tokio::test]
    async fn test_unknown_kind_returns_error() {
        let backend = MockBackend::new(); // no schemas
        let cache = init_cache(&backend).await;

        let result = delete(&backend, &cache, None, delete_request("default", "Ghost", "obj1"))
            .await;
        assert!(matches!(result, Err(DawnStoreError::UnknownResourceKind(_))));
    }

    /// Caller with Delete only on kind A cannot delete kind B.
    #[tokio::test]
    async fn test_delete_permission_on_wrong_kind_returns_forbidden() {
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_schema(permissive_schema("v1", "Secret"))
            .with_object(make_return_object("default", "v1", "Secret", "secret1"));
        let cache = init_cache(&backend).await;

        let caller = make_claims("default", "svc-a");
        cache.insert_permissions(
            "default",
            "svc-a",
            EffectivePermissions {
                namespaced: vec![GrantedScope {
                    api_version: "*".to_string(),
                    kinds: vec!["Car".to_string()], // Delete on Car only
                    verbs: [Verb::Delete].into_iter().collect(),
                    names: None,
                }],
                global: vec![],
            },
            vec![],
        );

        let result =
            delete(&backend, &cache, Some(&caller), delete_request("default", "Secret", "secret1"))
                .await;
        assert!(matches!(result, Err(DawnStoreError::Forbidden)));
        assert!(backend.contains("default", "Secret", "secret1"));
    }
}
