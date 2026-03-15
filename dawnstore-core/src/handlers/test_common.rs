/// Shared test infrastructure for handler unit tests.
///
/// Provides a `MockBackend` that implements `DawnstoreBackend` and a set
/// of helper constructors used across `apply_tests`, `get_tests`, and
/// `delete_tests`.
use std::collections::HashMap;
use std::future::Future;

use chrono::Utc;
use dawnstore_lib::{AllowedScope, ObjectAny, ResourceDefinition, ReturnObject};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::abstractions::{
    BackendGetObjectsFilter, ForeignKeyBehaviour, ForeignKeyType, DawnstoreBackend,
    ObjectRelation, RawForeignKeyConstraint, RawSchema,
};
use crate::cache::DawnstoreCache;
use crate::error::DawnStoreError;
use crate::cache::{GrantedScope, Verb};
use crate::rbac::helpers::object_string_id;
use crate::rbac::middleware::Claims;

// ── MockBackend ───────────────────────────────────────────────────────────────

pub struct MockBackend {
    pub schemas: Vec<RawSchema>,
    pub fk_constraints: Vec<RawForeignKeyConstraint>,
    /// Stored as serialised `ReturnObject<Value>`, keyed by `namespace/kind/name`.
    pub objects: std::sync::Mutex<HashMap<String, Value>>,
    /// Inbound FK references: target string ID → list of referencing string IDs.
    pub inbound_refs: std::sync::Mutex<HashMap<String, Vec<String>>>,
}

impl MockBackend {
    pub fn new() -> Self {
        Self {
            schemas: vec![],
            fk_constraints: vec![],
            objects: std::sync::Mutex::new(HashMap::new()),
            inbound_refs: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn with_schema(mut self, schema: RawSchema) -> Self {
        self.schemas.push(schema);
        self
    }

    pub fn with_fk(mut self, fk: RawForeignKeyConstraint) -> Self {
        self.fk_constraints.push(fk);
        self
    }

    pub fn with_object(self, obj: ReturnObject<Value>) -> Self {
        let key = object_string_id(&obj.namespace, &obj.kind, &obj.name);
        self.objects.lock().unwrap().insert(key, serde_json::to_value(&obj).unwrap());
        self
    }

    /// Register that the object at `(namespace, kind, name)` is referenced by
    /// `referencing_sid`. Used by tests that verify the delete-blocked path.
    pub fn with_inbound_reference(
        self,
        namespace: &str,
        kind: &str,
        name: &str,
        referencing_sid: impl Into<String>,
    ) -> Self {
        let target = object_string_id(namespace, kind, name);
        self.inbound_refs.lock().unwrap().entry(target).or_default().push(referencing_sid.into());
        self
    }

    pub fn contains(&self, namespace: &str, kind: &str, name: &str) -> bool {
        self.objects.lock().unwrap().contains_key(&object_string_id(namespace, kind, name))
    }
}

impl DawnstoreBackend for MockBackend {
    fn load_all_schemas(
        &self,
    ) -> impl Future<Output = Result<Vec<RawSchema>, DawnStoreError>> + Send {
        let schemas = self.schemas.clone();
        async move { Ok(schemas) }
    }

    fn load_all_foreign_key_constraints(
        &self,
    ) -> impl Future<Output = Result<Vec<RawForeignKeyConstraint>, DawnStoreError>> + Send {
        let fks = self.fk_constraints.clone();
        async move { Ok(fks) }
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
                .filter(|obj| matches_filter(obj, &filter_ns, &filter_kind, &filter_name, &filter_allowed))
                .collect()
        };
        async move { Ok(results) }
    }

    fn get_object(
        &self,
        namespace: &str,
        kind: &str,
        name: &str,
    ) -> impl Future<Output = Result<Option<ReturnObject<Value>>, DawnStoreError>> + Send {
        let key = object_string_id(namespace, kind, name);
        let result = {
            let objects = self.objects.lock().unwrap();
            objects.get(&key).map(|v| serde_json::from_value::<ReturnObject<Value>>(v.clone()).unwrap())
        };
        async move { Ok(result) }
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
    ) -> impl Future<Output = Result<Vec<ResourceDefinition>, DawnStoreError>> + Send {
        async move { Ok(vec![]) }
    }

    fn get_inbound_references(
        &self,
        namespace: &str,
        kind: &str,
        name: &str,
    ) -> impl Future<Output = Result<Vec<String>, DawnStoreError>> + Send {
        let target = object_string_id(namespace, kind, name);
        let refs = self.inbound_refs.lock().unwrap().get(&target).cloned().unwrap_or_default();
        async move { Ok(refs) }
    }

    fn seed_schemas(
        &self,
        _schemas: &[crate::abstractions::SchemaDefinition],
    ) -> impl Future<Output = Result<(), DawnStoreError>> + Send {
        async move { Ok(()) }
    }
}

fn matches_filter(
    obj: &ReturnObject<Value>,
    ns: &Option<String>,
    kind: &Option<String>,
    name: &Option<String>,
    allowed: &Option<Vec<AllowedScope>>,
) -> bool {
    ns.as_deref().map_or(true, |n| obj.namespace == n)
        && kind.as_deref().map_or(true, |k| obj.kind == k)
        && name.as_deref().map_or(true, |n| obj.name == n)
        && allowed.as_ref().map_or(true, |scopes| {
            if scopes.is_empty() {
                return false;
            }
            scopes.iter().any(|scope| {
                scope.namespace.as_deref().map_or(true, |ns| ns == obj.namespace)
                    && (scope.kind == "*" || scope.kind == obj.kind)
                    && scope.names.as_ref().map_or(true, |names| names.contains(&obj.name))
            })
        })
}

// ── Shared helper constructors ────────────────────────────────────────────────

pub fn make_claims(namespace: &str, sa_name: &str) -> Claims {
    Claims {
        sub: sa_name.to_string(),
        namespace: namespace.to_string(),
        token_name: "test-token".to_string(),
        token_id: Uuid::new_v4(),
        exp: u64::MAX,
    }
}

pub fn permissive_schema(api_version: &str, kind: &str) -> RawSchema {
    RawSchema {
        api_version: api_version.to_string(),
        kind: kind.to_string(),
        aliases: vec![],
        json_schema: r#"{"type": "object"}"#.to_string(),
    }
}

pub fn schema_with_aliases(api_version: &str, kind: &str, aliases: &[&str]) -> RawSchema {
    RawSchema {
        api_version: api_version.to_string(),
        kind: kind.to_string(),
        aliases: aliases.iter().map(|s| s.to_string()).collect(),
        json_schema: r#"{"type": "object"}"#.to_string(),
    }
}

pub fn make_return_object(
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

pub fn make_fk(
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

pub fn wildcard_grant(verbs: &[Verb]) -> GrantedScope {
    GrantedScope {
        api_version: "*".to_string(),
        kinds: vec!["*".to_string()],
        verbs: verbs.iter().copied().collect(),
        names: None,
    }
}

pub fn kind_grant(kind: &str, verbs: &[Verb]) -> GrantedScope {
    GrantedScope {
        api_version: "*".to_string(),
        kinds: vec![kind.to_string()],
        verbs: verbs.iter().copied().collect(),
        names: None,
    }
}

pub async fn init_cache(backend: &MockBackend) -> DawnstoreCache {
    DawnstoreCache::init(backend).await.unwrap()
}
