#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::future::Future;

    use chrono::Utc;
    use serde_json::{json, Value};
    use uuid::Uuid;

    use crate::abstractions::{
        BackendGetObjectsFilter, ForeignKeyBehaviour, ForeignKeyType, NewDawnStoreBackend,
        ObjectRelation, RawForeignKeyConstraint, RawSchema,
    };
    use crate::rbac::helpers::object_string_id;
    use crate::cache::DawnstoreCache;
    use crate::error::DawnStoreError;
    use crate::handlers::apply::apply;
    use crate::rbac::cache::{EffectivePermissions, GrantedScope, Verb};
    use crate::rbac::middleware::Claims;
    use dawnstore_lib::{ObjectAny, ReturnObject};

    // ── MockBackend ───────────────────────────────────────────────────────────

    struct MockBackend {
        schemas: Vec<RawSchema>,
        fk_constraints: Vec<RawForeignKeyConstraint>,
        /// Stored as serialised `ReturnObject<Value>`, keyed by `namespace/kind/name`.
        objects: std::sync::Mutex<HashMap<String, Value>>,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                schemas: vec![],
                fk_constraints: vec![],
                objects: std::sync::Mutex::new(HashMap::new()),
            }
        }

        fn with_schema(mut self, schema: RawSchema) -> Self {
            self.schemas.push(schema);
            self
        }

        fn with_fk(mut self, fk: RawForeignKeyConstraint) -> Self {
            self.fk_constraints.push(fk);
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
            let fks = self.fk_constraints.clone();
            async move { Ok(fks) }
        }

        fn get_objects(
            &self,
            _filter: &BackendGetObjectsFilter,
        ) -> impl Future<Output = Result<Vec<ReturnObject<Value>>, DawnStoreError>> + Send
        {
            // Return empty — permissions are injected directly via cache.insert_permissions().
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
            // Drop the lock before the async block so the guard doesn't cross an await.
            let result = {
                let objects = self.objects.lock().unwrap();
                objects
                    .get(&key)
                    .map(|v| serde_json::from_value::<ReturnObject<Value>>(v.clone()).unwrap())
            };
            async move { Ok(result) }
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

        fn upsert_objects(
            &self,
            objects: Vec<ObjectAny>,
            _relations: Vec<ObjectRelation>,
        ) -> impl Future<Output = Result<Vec<ReturnObject<Value>>, DawnStoreError>> + Send {
            let results: Vec<ReturnObject<Value>> = objects
                .into_iter()
                .map(|obj| ReturnObject {
                    id: Uuid::new_v4(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    annotations: obj.annotations,
                    labels: obj.labels,
                    namespace: obj.namespace.unwrap_or_else(|| "default".to_string()),
                    api_version: obj.api_version.unwrap_or_default(),
                    kind: obj.kind.unwrap_or_default(),
                    name: obj.name,
                    spec: obj.spec,
                })
                .collect();
            async move { Ok(results) }
        }

        fn get_resource_definitions(
            &self,
        ) -> impl Future<Output = Result<Vec<dawnstore_lib::ResourceDefinition>, DawnStoreError>> + Send
        {
            async move { Ok(vec![]) }
        }
    }

    // ── Test helpers ──────────────────────────────────────────────────────────

    fn make_claims(namespace: &str, sa_name: &str) -> Claims {
        Claims {
            sub: sa_name.to_string(),
            namespace: namespace.to_string(),
            token_name: "test-token".to_string(),
            token_id: Uuid::new_v4(),
            exp: u64::MAX,
        }
    }

    /// A permissive schema that accepts any object.
    fn permissive_schema(api_version: &str, kind: &str) -> RawSchema {
        RawSchema {
            api_version: api_version.to_string(),
            kind: kind.to_string(),
            aliases: vec![],
            json_schema: r#"{"type": "object"}"#.to_string(),
        }
    }

    /// A strict schema that requires an integer `value` field and nothing else.
    fn strict_schema(api_version: &str, kind: &str) -> RawSchema {
        RawSchema {
            api_version: api_version.to_string(),
            kind: kind.to_string(),
            aliases: vec![],
            json_schema: r#"{"type":"object","required":["value"],"properties":{"value":{"type":"integer"}},"additionalProperties":false}"#.to_string(),
        }
    }

    fn make_fk(
        api_version: &str,
        kind: &str,
        key_path: &str,
        ty: ForeignKeyType,
        foreign_key_kind: Option<&str>,
    ) -> RawForeignKeyConstraint {
        RawForeignKeyConstraint {
            id: Uuid::new_v4(),
            api_version: api_version.to_string(),
            kind: kind.to_string(),
            key_path: key_path.to_string(),
            ty,
            behaviour: ForeignKeyBehaviour::Fill,
            foreign_key_kind: foreign_key_kind.map(|s| s.to_string()),
            parent_key_path: None,
        }
    }

    fn make_return_object(
        namespace: &str,
        api_version: &str,
        kind: &str,
        name: &str,
        spec: Value,
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
            spec,
        }
    }

    /// Build a single-object apply input. `spec` fields are merged at the top level
    /// because `Object<T>` uses `#[serde(flatten)]` on its spec.
    fn single_object(
        namespace: &str,
        api_version: &str,
        kind: &str,
        name: &str,
        spec: Value,
    ) -> Value {
        let mut obj = json!({
            "namespace": namespace,
            "api_version": api_version,
            "kind": kind,
            "name": name,
        });
        if let (Some(obj_map), Some(spec_map)) = (obj.as_object_mut(), spec.as_object()) {
            for (k, v) in spec_map {
                obj_map.insert(k.clone(), v.clone());
            }
        }
        obj
    }

    /// Grants `verbs` on all api_versions and all kinds.
    fn wildcard_grant(verbs: &[Verb]) -> GrantedScope {
        GrantedScope {
            api_version: "*".to_string(),
            kinds: vec!["*".to_string()],
            verbs: verbs.iter().copied().collect(),
            names: None,
        }
    }

    /// Grants `verbs` on all api_versions for a specific `kind`.
    fn kind_grant(kind: &str, verbs: &[Verb]) -> GrantedScope {
        GrantedScope {
            api_version: "*".to_string(),
            kinds: vec![kind.to_string()],
            verbs: verbs.iter().copied().collect(),
            names: None,
        }
    }

    async fn init_cache(backend: &MockBackend) -> DawnstoreCache {
        DawnstoreCache::init(backend).await.unwrap()
    }

    // ── Positive tests ────────────────────────────────────────────────────────

    /// Unauthenticated (superadmin) path skips all permission checks.
    #[tokio::test]
    async fn test_unauthenticated_apply_succeeds() {
        let backend = MockBackend::new().with_schema(permissive_schema("v1", "Car"));
        let cache = init_cache(&backend).await;

        let input = single_object("default", "v1", "Car", "my-car", json!({}));
        let result = apply(&backend, &cache, None, input).await;
        assert!(result.is_ok());
    }

    /// Authenticated caller with Apply grant succeeds.
    #[tokio::test]
    async fn test_authenticated_apply_with_permission_succeeds() {
        let backend = MockBackend::new().with_schema(permissive_schema("v1", "Car"));
        let cache = init_cache(&backend).await;

        let caller = make_claims("default", "svc-a");
        cache.insert_permissions(
            "default",
            "svc-a",
            EffectivePermissions { namespaced: vec![wildcard_grant(&[Verb::Apply])], global: vec![] },
            vec![],
        );

        let input = single_object("default", "v1", "Car", "my-car", json!({}));
        let result = apply(&backend, &cache, Some(&caller), input).await;
        assert!(result.is_ok());
    }

    /// Array input applies all objects in the list.
    #[tokio::test]
    async fn test_array_input_applies_multiple_objects() {
        let backend = MockBackend::new().with_schema(permissive_schema("v1", "Car"));
        let cache = init_cache(&backend).await;

        let input = json!([
            {"namespace": "default", "api_version": "v1", "kind": "Car", "name": "car1"},
            {"namespace": "default", "api_version": "v1", "kind": "Car", "name": "car2"},
        ]);
        let result = apply(&backend, &cache, None, input).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 2);
    }

    /// List wrapper fills in `object_kind` and `object_api_version` for items that
    /// omit their own kind / api_version.
    #[tokio::test]
    async fn test_list_with_implied_kind_applies_successfully() {
        let backend = MockBackend::new().with_schema(permissive_schema("v1", "Car"));
        let cache = init_cache(&backend).await;

        let input = json!({
            "kind": "List",
            "object_kind": "Car",
            "object_api_version": "v1",
            "list": [
                {"namespace": "default", "name": "car1"},
                {"namespace": "default", "name": "car2"},
            ]
        });
        let result = apply(&backend, &cache, None, input).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 2);
    }

    /// A required FK pointing to an existing object is resolved without error.
    #[tokio::test]
    async fn test_required_fk_to_existing_object_succeeds() {
        let fk = make_fk("v1", "Car", "owner", ForeignKeyType::One, Some("Owner"));
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_schema(permissive_schema("v1", "Owner"))
            .with_fk(fk)
            .with_object(make_return_object("default", "v1", "Owner", "alice", json!({})));
        let cache = init_cache(&backend).await;

        let input = single_object("default", "v1", "Car", "car1", json!({"owner": "alice"}));
        let result = apply(&backend, &cache, None, input).await;
        assert!(result.is_ok());
    }

    /// An optional FK that is absent (null / missing) is silently ignored.
    #[tokio::test]
    async fn test_optional_fk_absent_succeeds() {
        let fk = make_fk("v1", "Car", "owner", ForeignKeyType::OneOptional, Some("Owner"));
        let backend =
            MockBackend::new().with_schema(permissive_schema("v1", "Car")).with_fk(fk);
        let cache = init_cache(&backend).await;

        // Spec has no `owner` field.
        let input = single_object("default", "v1", "Car", "car1", json!({}));
        let result = apply(&backend, &cache, None, input).await;
        assert!(result.is_ok());
    }

    /// FK graph walk follows chains: child → parent-a → grandparent-b.
    #[tokio::test]
    async fn test_nested_fk_graph_walk_succeeds() {
        let fk = make_fk("v1", "Container", "parent", ForeignKeyType::OneOptional, Some("Container"));
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Container"))
            .with_fk(fk)
            .with_object(make_return_object(
                "default",
                "v1",
                "Container",
                "parent-a",
                json!({"parent": "grandparent-b"}),
            ))
            .with_object(make_return_object(
                "default",
                "v1",
                "Container",
                "grandparent-b",
                json!({}),
            ));
        let cache = init_cache(&backend).await;

        let input =
            single_object("default", "v1", "Container", "child", json!({"parent": "parent-a"}));
        let result = apply(&backend, &cache, None, input).await;
        assert!(result.is_ok());
    }

    /// Caller has both Apply and Get → apply with FK succeeds.
    #[tokio::test]
    async fn test_apply_and_get_on_fk_target_both_granted_succeeds() {
        let fk = make_fk("v1", "Car", "owner", ForeignKeyType::One, Some("Owner"));
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_schema(permissive_schema("v1", "Owner"))
            .with_fk(fk)
            .with_object(make_return_object("default", "v1", "Owner", "alice", json!({})));
        let cache = init_cache(&backend).await;

        let caller = make_claims("default", "svc-a");
        cache.insert_permissions(
            "default",
            "svc-a",
            EffectivePermissions {
                namespaced: vec![wildcard_grant(&[Verb::Apply, Verb::Get])],
                global: vec![],
            },
            vec![],
        );

        let input = single_object("default", "v1", "Car", "car1", json!({"owner": "alice"}));
        let result = apply(&backend, &cache, Some(&caller), input).await;
        assert!(result.is_ok());
    }

    // ── Negative / permission tests ───────────────────────────────────────────

    /// Caller without Apply permission receives Forbidden.
    #[tokio::test]
    async fn test_no_apply_permission_returns_forbidden() {
        let backend = MockBackend::new().with_schema(permissive_schema("v1", "Car"));
        let cache = init_cache(&backend).await;

        let caller = make_claims("default", "svc-a");
        // No permissions injected.

        let input = single_object("default", "v1", "Car", "my-car", json!({}));
        let result = apply(&backend, &cache, Some(&caller), input).await;
        assert!(matches!(result, Err(DawnStoreError::Forbidden)));
    }

    /// Caller has Apply on the object kind but not Get on the FK target → Forbidden.
    #[tokio::test]
    async fn test_apply_granted_but_no_get_on_fk_target_returns_forbidden() {
        let fk = make_fk("v1", "Car", "owner", ForeignKeyType::One, Some("Owner"));
        let backend = MockBackend::new()
            .with_schema(permissive_schema("v1", "Car"))
            .with_fk(fk)
            .with_object(make_return_object("default", "v1", "Owner", "alice", json!({})));
        let cache = init_cache(&backend).await;

        let caller = make_claims("default", "svc-a");
        cache.insert_permissions(
            "default",
            "svc-a",
            EffectivePermissions {
                namespaced: vec![kind_grant("Car", &[Verb::Apply])], // no Get anywhere
                global: vec![],
            },
            vec![],
        );

        let input = single_object("default", "v1", "Car", "car1", json!({"owner": "alice"}));
        let result = apply(&backend, &cache, Some(&caller), input).await;
        assert!(matches!(result, Err(DawnStoreError::Forbidden)));
    }

    // ── Negative / input-parsing tests ────────────────────────────────────────

    /// A JSON number as root input is rejected.
    #[tokio::test]
    async fn test_invalid_root_value_returns_error() {
        let backend = MockBackend::new();
        let cache = init_cache(&backend).await;

        let result = apply(&backend, &cache, None, json!(42)).await;
        assert!(matches!(result, Err(DawnStoreError::InvalidRootInputObject)));
    }

    /// A JSON boolean as root input is rejected.
    #[tokio::test]
    async fn test_invalid_root_bool_returns_error() {
        let backend = MockBackend::new();
        let cache = init_cache(&backend).await;

        let result = apply(&backend, &cache, None, json!(true)).await;
        assert!(matches!(result, Err(DawnStoreError::InvalidRootInputObject)));
    }

    /// A single-object input without a `kind` field is rejected.
    #[tokio::test]
    async fn test_single_object_missing_kind_returns_error() {
        let backend = MockBackend::new();
        let cache = init_cache(&backend).await;

        let result = apply(&backend, &cache, None, json!({"name": "foo", "api_version": "v1"}))
            .await;
        assert!(matches!(result, Err(DawnStoreError::InvalidInputObjectMissingKindField)));
    }

    /// A List wrapper without the `list` field is rejected.
    #[tokio::test]
    async fn test_list_missing_list_field_returns_error() {
        let backend = MockBackend::new();
        let cache = init_cache(&backend).await;

        let result = apply(&backend, &cache, None, json!({"kind": "List", "object_kind": "Car"}))
            .await;
        assert!(matches!(result, Err(DawnStoreError::InvalidInputObjectMissingListFieldOfList)));
    }

    // ── Negative / schema-validation tests ───────────────────────────────────

    /// An object whose spec does not satisfy its registered schema is rejected.
    #[tokio::test]
    async fn test_schema_validation_failure_returns_error() {
        let backend = MockBackend::new().with_schema(strict_schema("v1", "Strict"));
        let cache = init_cache(&backend).await;

        // Spec is missing the required `value` field.
        let input = single_object("default", "v1", "Strict", "obj1", json!({}));
        let result = apply(&backend, &cache, None, input).await;
        assert!(matches!(result, Err(DawnStoreError::ObjectValidationError { .. })));
    }

    /// An object whose kind has no registered schema is rejected.
    #[tokio::test]
    async fn test_no_schema_for_kind_returns_error() {
        let backend = MockBackend::new(); // no schemas
        let cache = init_cache(&backend).await;

        let input = single_object("default", "v1", "Unknown", "obj1", json!({}));
        let result = apply(&backend, &cache, None, input).await;
        assert!(matches!(result, Err(DawnStoreError::NoSchemaForObjectFound { .. })));
    }

    // ── Negative / FK-validation tests ────────────────────────────────────────

    /// A required (`One`) FK field that is absent in the spec is rejected.
    #[tokio::test]
    async fn test_required_fk_field_absent_returns_error() {
        let fk = make_fk("v1", "Car", "owner", ForeignKeyType::One, Some("Owner"));
        let backend =
            MockBackend::new().with_schema(permissive_schema("v1", "Car")).with_fk(fk);
        let cache = init_cache(&backend).await;

        // Spec has no `owner` field.
        let input = single_object("default", "v1", "Car", "car1", json!({}));
        let result = apply(&backend, &cache, None, input).await;
        assert!(matches!(
            result,
            Err(DawnStoreError::ObjectValidationMissingForeignKeyEntry { .. })
        ));
    }

    /// A required FK whose target does not exist in the backend is rejected.
    #[tokio::test]
    async fn test_required_fk_target_not_found_returns_error() {
        let fk = make_fk("v1", "Car", "owner", ForeignKeyType::One, Some("Owner"));
        let backend =
            MockBackend::new().with_schema(permissive_schema("v1", "Car")).with_fk(fk);
        let cache = init_cache(&backend).await;

        let input =
            single_object("default", "v1", "Car", "car1", json!({"owner": "nonexistent"}));
        let result = apply(&backend, &cache, None, input).await;
        assert!(matches!(
            result,
            Err(DawnStoreError::ObjectValidationForeignKeyNotFound { .. })
        ));
    }

    /// A FK value with too many path segments (4+) is rejected.
    #[tokio::test]
    async fn test_fk_invalid_format_too_many_segments_returns_error() {
        let fk = make_fk("v1", "Car", "owner", ForeignKeyType::One, Some("Owner"));
        let backend =
            MockBackend::new().with_schema(permissive_schema("v1", "Car")).with_fk(fk);
        let cache = init_cache(&backend).await;

        // "a/b/c/d" has four segments → format error.
        let input = single_object("default", "v1", "Car", "car1", json!({"owner": "a/b/c/d"}));
        let result = apply(&backend, &cache, None, input).await;
        assert!(matches!(
            result,
            Err(DawnStoreError::ObjectValidationWrongForeignKeyEntryFormat { .. })
        ));
    }

    /// A FK value whose kind segment does not match the constraint's `foreign_key_kind` is rejected.
    #[tokio::test]
    async fn test_fk_wrong_kind_returns_error() {
        let fk = make_fk("v1", "Car", "owner", ForeignKeyType::One, Some("Owner"));
        let backend =
            MockBackend::new().with_schema(permissive_schema("v1", "Car")).with_fk(fk);
        let cache = init_cache(&backend).await;

        // "WrongKind/alice" has kind segment "WrongKind" but constraint expects "Owner".
        let input =
            single_object("default", "v1", "Car", "car1", json!({"owner": "WrongKind/alice"}));
        let result = apply(&backend, &cache, None, input).await;
        assert!(matches!(
            result,
            Err(DawnStoreError::ObjectValidationWrongForeignKeyEntryKind { .. })
        ));
    }
}
