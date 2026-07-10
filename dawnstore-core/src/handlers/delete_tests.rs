#[cfg(test)]
mod tests {
    use dawnstore_lib::DeleteObject;
    use uuid::Uuid;

    use crate::error::DawnStoreError;
    use crate::handlers::delete::delete;
    use crate::handlers::test_common::{
        MockBackend, init_cache, make_claims, make_return_object, permissive_schema,
        schema_with_aliases, wildcard_grant,
    };
    use crate::cache::{EffectivePermissions, GrantedScope, Verb};
    use crate::rbac::constants::{API_VERSION_V1, KIND_ROLE_BINDING};
    use crate::rbac::helpers::object_string_id;
    use crate::rbac::middleware::Claims;

    // ── Test-local helpers ────────────────────────────────────────────────────

    fn wildcard_delete() -> GrantedScope {
        wildcard_grant(&[Verb::Delete])
    }

    fn delete_request(namespace: &str, kind: &str, name: &str) -> DeleteObject {
        DeleteObject {
            namespace: Some(namespace.to_string()),
            api_version: None,
            kind: kind.to_string(),
            name: name.to_string(),
        }
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
            delete(&backend, &cache, Some(&caller), delete_request("default", "Car", "car1")).await;
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

        let result = delete(
            &backend,
            &cache,
            None,
            DeleteObject {
                namespace: Some("default".to_string()),
                api_version: None,
                kind: "cars".to_string(),
                name: "car1".to_string(),
            },
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
            iss: "dawnstore".to_string(),
            aud: "dawnstore".to_string(),
        };

        let result =
            delete(&backend, &cache, Some(&caller), delete_request("default", "Car", "car1")).await;
        assert!(result.is_ok());
    }

    /// Deleting an RBAC resource evicts permission-cache entries derived from it.
    #[tokio::test]
    async fn test_delete_rbac_resource_invalidates_permission_cache() {
        let backend = MockBackend::new()
            .with_schema(permissive_schema(API_VERSION_V1, KIND_ROLE_BINDING));
        let cache = init_cache(&backend).await;

        let rb_sid = object_string_id("default", KIND_ROLE_BINDING, "my-rb");
        cache.insert_permissions(
            "default",
            "svc-a",
            EffectivePermissions { namespaced: vec![wildcard_delete()], global: vec![] },
            vec![rb_sid],
        );
        assert!(cache.get_permissions("default", "svc-a").is_some());

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

    /// Deleting an object that is still referenced by another object is blocked.
    #[tokio::test]
    async fn test_delete_blocked_when_inbound_references_exist() {
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "ServiceAccount"))
            .with_schema(permissive_schema("v1", "RoleBinding"))
            .with_object(make_return_object("demo", "v1", "ServiceAccount", "alice"))
            .with_inbound_reference(
                "demo",
                "ServiceAccount",
                "alice",
                "demo/RoleBinding/bind-alice",
            );
        let cache = init_cache(&backend).await;

        let result = delete(
            &backend,
            &cache,
            None,
            delete_request("demo", "ServiceAccount", "alice"),
        )
        .await;
        assert!(
            matches!(result, Err(DawnStoreError::DeleteBlockedByReferences { .. })),
            "delete must be blocked when referencing objects exist, got: {result:?}"
        );
        // Object must still be present.
        assert!(backend.contains("demo", "ServiceAccount", "alice"));
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

        let result =
            delete(&backend, &cache, Some(&caller), delete_request("default", "Car", "car1")).await;
        assert!(matches!(result, Err(DawnStoreError::Forbidden)));
        assert!(backend.contains("default", "Car", "car1"));
    }

    /// Deleting an unregistered kind returns UnknownResourceKind.
    #[tokio::test]
    async fn test_unknown_kind_returns_error() {
        let backend = MockBackend::new();
        let cache = init_cache(&backend).await;

        let result =
            delete(&backend, &cache, None, delete_request("default", "Ghost", "obj1")).await;
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
                    kinds: vec!["Car".to_string()],
                    verbs: [Verb::Delete].into_iter().collect(),
                    names: None,
                }],
                global: vec![],
            },
            vec![],
        );

        let result = delete(
            &backend,
            &cache,
            Some(&caller),
            delete_request("default", "Secret", "secret1"),
        )
        .await;
        assert!(matches!(result, Err(DawnStoreError::Forbidden)));
        assert!(backend.contains("default", "Secret", "secret1"));
    }
}
