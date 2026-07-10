#[cfg(test)]
mod tests {
    use dawnstore_lib::GetObjectsFilter;
    use uuid::Uuid;

    use crate::error::DawnStoreError;
    use crate::handlers::get::get;
    use crate::handlers::test_common::{
        MockBackend, init_cache, make_claims, make_return_object, permissive_schema,
        schema_with_aliases, wildcard_grant,
    };
    use crate::cache::{EffectivePermissions, GrantedScope, Verb};
    use crate::rbac::middleware::Claims;

    // ── Test-local helpers ────────────────────────────────────────────────────

    fn wildcard_get() -> GrantedScope {
        wildcard_grant(&[Verb::Get])
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
        let backend = MockBackend::new();
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

        let result = get(&backend, &cache, Some(&caller), filter_kind("Car")).await.unwrap();
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
                namespaced: vec![kind_get("Car")],
                global: vec![],
            },
            vec![],
        );

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
                global: vec![wildcard_get()],
            },
            vec![],
        );

        let result = get(&backend, &cache, Some(&caller), filter_kind("Car")).await.unwrap();
        assert_eq!(result.len(), 2);
    }

    /// Unknown non-alias returns error.
    #[tokio::test]
    async fn test_unknown_alias_returns_error() {
        let backend = MockBackend::new().with_schema(schema_with_aliases("v1", "Car", &["cars"]));
        let cache = init_cache(&backend).await;

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

        let caller = make_claims("ns-a", "svc-a");
        cache.insert_permissions(
            "ns-a",
            "svc-a",
            EffectivePermissions { namespaced: vec![wildcard_get()], global: vec![] },
            vec![],
        );

        let result = get(&backend, &cache, Some(&caller), filter_kind("Car")).await.unwrap();
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
            iss: "dawnstore".to_string(),
            aud: "dawnstore".to_string(),
        };

        let result = get(&backend, &cache, Some(&caller), filter_kind("Car")).await.unwrap();
        assert_eq!(result.len(), 2);
    }
}
