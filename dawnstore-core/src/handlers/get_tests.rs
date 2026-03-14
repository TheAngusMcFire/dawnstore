#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::future::Future;

    use chrono::Utc;
    use dawnstore_lib::{GetObjectsFilter, ObjectAny, ReturnObject};
    use serde_json::{json, Value};
    use uuid::Uuid;

    use crate::abstractions::{
        BackendGetObjectsFilter, NewDawnStoreBackend, ObjectRelation, RawForeignKeyConstraint,
        RawSchema,
    };
    use crate::cache::DawnstoreCache;
    use crate::error::DawnStoreError;
    use crate::handlers::get::get;
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
            filter: &BackendGetObjectsFilter,
        ) -> impl Future<Output = Result<Vec<ReturnObject<Value>>, DawnStoreError>> + Send {
            let filter_ns = filter.namespace.clone();
            let filter_kind = filter.kind.clone();
            let filter_name = filter.name.clone();
            let filter_allowed = filter.allowed.clone();

            let results: Vec<ReturnObject<Value>> = {
                let map = self.objects.lock().unwrap();
                map.values()
                    .map(|v| serde_json::from_value::<ReturnObject<Value>>(v.clone()).unwrap())
                    .filter(|obj| {
                        filter_ns.as_deref().map_or(true, |ns| obj.namespace == ns)
                            && filter_kind.as_deref().map_or(true, |k| obj.kind == k)
                            && filter_name.as_deref().map_or(true, |n| obj.name == n)
                            && filter_allowed.as_ref().map_or(true, |allowed| {
                                if allowed.is_empty() {
                                    return false;
                                }
                                allowed.iter().any(|scope| {
                                    scope
                                        .namespace
                                        .as_deref()
                                        .map_or(true, |ns| ns == obj.namespace)
                                        && (scope.kind == "*" || scope.kind == obj.kind)
                                        && scope.names.as_ref().map_or(true, |names| {
                                            names.contains(&obj.name)
                                        })
                                })
                            })
                    })
                    .collect()
            };
            async move { Ok(results) }
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

        fn get_resource_definitions(
            &self,
        ) -> impl Future<Output = Result<Vec<dawnstore_lib::ResourceDefinition>, DawnStoreError>> + Send
        {
            async move { Ok(vec![]) }
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

    fn wildcard_get() -> GrantedScope {
        GrantedScope {
            api_version: "*".to_string(),
            kinds: vec!["*".to_string()],
            verbs: [Verb::Get].into_iter().collect(),
            names: None,
        }
    }

    fn kind_get(kind: &str) -> GrantedScope {
        GrantedScope {
            api_version: "*".to_string(),
            kinds: vec![kind.to_string()],
            verbs: [Verb::Get].into_iter().collect(),
            names: None,
        }
    }

    fn filter_kind(kind: &str) -> GetObjectsFilter {
        GetObjectsFilter { kind: Some(kind.to_string()), ..Default::default() }
    }

    async fn init_cache(backend: &MockBackend) -> DawnstoreCache {
        DawnstoreCache::init(backend).await.unwrap()
    }

    // ── Positive tests ────────────────────────────────────────────────────────

    /// Unauthenticated (superadmin) path returns all matching objects.
    #[tokio::test]
    async fn test_unauthenticated_get_returns_all_objects() {
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_object(make_return_object("default", "v1", "Car", "car1"))
            .with_object(make_return_object("default", "v1", "Car", "car2"));
        let cache = init_cache(&backend).await;

        let result = get(&backend, &cache, None, filter_kind("Car")).await.unwrap();
        assert_eq!(result.len(), 2);
    }

    /// Authenticated caller with wildcard Get sees all objects.
    #[tokio::test]
    async fn test_authenticated_get_with_permission_returns_objects() {
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_object(make_return_object("default", "v1", "Car", "car1"));
        let cache = init_cache(&backend).await;

        let caller = make_claims("default", "svc-a");
        cache.insert_permissions(
            "default",
            "svc-a",
            EffectivePermissions { namespaced: vec![wildcard_get()], global: vec![] },
            vec![],
        );

        let result = get(&backend, &cache, Some(&caller), filter_kind("Car")).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "car1");
    }

    /// A kind alias is resolved to the canonical kind before querying.
    #[tokio::test]
    async fn test_kind_alias_is_resolved() {
        let backend = MockBackend::new()
            .with_schema(schema_with_aliases("v1", "Car", &["cars", "automobile"]))
            .with_object(make_return_object("default", "v1", "Car", "car1"));
        let cache = init_cache(&backend).await;

        // Query using the alias "cars".
        let result = get(&backend, &cache, None, filter_kind("cars")).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].kind, "Car");
    }

    /// Filtering by namespace returns only objects in that namespace.
    #[tokio::test]
    async fn test_filter_by_namespace() {
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_object(make_return_object("ns-a", "v1", "Car", "car1"))
            .with_object(make_return_object("ns-b", "v1", "Car", "car2"));
        let cache = init_cache(&backend).await;

        let filter = GetObjectsFilter {
            namespace: Some("ns-a".to_string()),
            kind: Some("Car".to_string()),
            ..Default::default()
        };
        let result = get(&backend, &cache, None, filter).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].namespace, "ns-a");
    }

    /// Filtering by name returns only the matching object.
    #[tokio::test]
    async fn test_filter_by_name() {
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_object(make_return_object("default", "v1", "Car", "car1"))
            .with_object(make_return_object("default", "v1", "Car", "car2"));
        let cache = init_cache(&backend).await;

        let filter = GetObjectsFilter {
            kind: Some("Car".to_string()),
            name: Some("car1".to_string()),
            ..Default::default()
        };
        let result = get(&backend, &cache, None, filter).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "car1");
    }

    /// No filter returns all objects in the store.
    #[tokio::test]
    async fn test_no_filter_returns_all() {
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_schema(permissive_schema("v1", "Owner"))
            .with_object(make_return_object("default", "v1", "Car", "car1"))
            .with_object(make_return_object("default", "v1", "Owner", "alice"));
        let cache = init_cache(&backend).await;

        let result = get(&backend, &cache, None, GetObjectsFilter::default()).await.unwrap();
        assert_eq!(result.len(), 2);
    }

    // ── Negative tests ────────────────────────────────────────────────────────

    /// Querying an unregistered kind returns UnknownResourceKind.
    #[tokio::test]
    async fn test_unknown_kind_returns_error() {
        let backend = MockBackend::new(); // no schemas
        let cache = init_cache(&backend).await;

        let result = get(&backend, &cache, None, filter_kind("Unknown")).await;
        assert!(matches!(result, Err(DawnStoreError::UnknownResourceKind(_))));
    }

    /// Authenticated caller with no Get grants receives an empty result, not Forbidden.
    #[tokio::test]
    async fn test_no_get_permission_returns_empty_not_forbidden() {
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_object(make_return_object("default", "v1", "Car", "car1"));
        let cache = init_cache(&backend).await;

        let caller = make_claims("default", "svc-a");
        // No permissions injected — SA has no grants at all.

        let result =
            get(&backend, &cache, Some(&caller), filter_kind("Car")).await.unwrap();
        // Must return empty, not Forbidden.
        assert!(result.is_empty());
    }

    /// Caller with Get on kind A does not see kind B objects.
    #[tokio::test]
    async fn test_get_permission_on_one_kind_does_not_expose_other() {
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_schema(permissive_schema("v1", "Secret"))
            .with_object(make_return_object("default", "v1", "Car", "car1"))
            .with_object(make_return_object("default", "v1", "Secret", "secret1"));
        let cache = init_cache(&backend).await;

        let caller = make_claims("default", "svc-a");
        cache.insert_permissions(
            "default",
            "svc-a",
            EffectivePermissions {
                namespaced: vec![kind_get("Car")], // Get on Car only
                global: vec![],
            },
            vec![],
        );

        // Query for all objects — RBAC should filter out Secret.
        let result =
            get(&backend, &cache, Some(&caller), GetObjectsFilter::default()).await.unwrap();
        assert!(result.iter().all(|o| o.kind == "Car"), "should not see Secret objects");
    }

    /// Global Get grant allows access to objects in any namespace.
    #[tokio::test]
    async fn test_global_get_grant_accesses_any_namespace() {
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_object(make_return_object("ns-a", "v1", "Car", "car-a"))
            .with_object(make_return_object("ns-b", "v1", "Car", "car-b"));
        let cache = init_cache(&backend).await;

        let caller = make_claims("ns-a", "svc-global");
        cache.insert_permissions(
            "ns-a",
            "svc-global",
            EffectivePermissions {
                namespaced: vec![],
                global: vec![wildcard_get()], // global grant
            },
            vec![],
        );

        let result =
            get(&backend, &cache, Some(&caller), filter_kind("Car")).await.unwrap();
        assert_eq!(result.len(), 2);
    }

    /// Querying with an unknown alias via a registered alias resolves correctly
    /// but unknown non-alias returns error.
    #[tokio::test]
    async fn test_unknown_alias_returns_error() {
        let backend =
            MockBackend::new().with_schema(schema_with_aliases("v1", "Car", &["cars"]));
        let cache = init_cache(&backend).await;

        // "automobiles" is not an alias for Car.
        let result = get(&backend, &cache, None, filter_kind("automobiles")).await;
        assert!(matches!(result, Err(DawnStoreError::UnknownResourceKind(_))));
    }

    /// Namespaced Get grant only exposes the SA's own namespace, not others.
    #[tokio::test]
    async fn test_namespaced_get_grant_scoped_to_sa_namespace() {
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_object(make_return_object("ns-a", "v1", "Car", "car-a"))
            .with_object(make_return_object("ns-b", "v1", "Car", "car-b"));
        let cache = init_cache(&backend).await;

        // SA is in ns-a with a namespaced Get grant.
        let caller = make_claims("ns-a", "svc-a");
        cache.insert_permissions(
            "ns-a",
            "svc-a",
            EffectivePermissions { namespaced: vec![wildcard_get()], global: vec![] },
            vec![],
        );

        let result = get(&backend, &cache, Some(&caller), filter_kind("Car")).await.unwrap();
        // Namespaced grant applies only to ns-a; ns-b objects are excluded.
        assert!(result.iter().all(|o| o.namespace == "ns-a"));
    }

    /// Superadmin (system/serviceaccount/superadmin) gets unrestricted results
    /// even though `caller` is Some.
    #[tokio::test]
    async fn test_superadmin_caller_gets_unrestricted_results() {
        use crate::rbac::constants::{SA_SUPERADMIN, SYSTEM_NAMESPACE};

        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_object(make_return_object("default", "v1", "Car", "car1"))
            .with_object(make_return_object("secret-ns", "v1", "Car", "car2"));
        let cache = init_cache(&backend).await;

        let caller = Claims {
            sub: SA_SUPERADMIN.to_string(),
            namespace: SYSTEM_NAMESPACE.to_string(),
            token_name: "bootstrap".to_string(),
            token_id: Uuid::new_v4(),
            exp: u64::MAX,
        };

        // No permissions injected — but superadmin bypasses all checks.
        let result =
            get(&backend, &cache, Some(&caller), filter_kind("Car")).await.unwrap();
        assert_eq!(result.len(), 2);
    }
}
