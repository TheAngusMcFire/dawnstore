use std::collections::HashMap;
use std::future::Future;
use std::sync::RwLock;

pub use dawnstore_lib::*;
use schemars::JsonSchema;
use uuid::Uuid;

use crate::error::DawnStoreError;

// ── Foreign key types ─────────────────────────────────────────────────────────

pub struct ForeignKey {
    pub path: String,
    pub parent_path: Option<String>,
    pub ty: ForeignKeyType,
    pub behaviour: ForeignKeyBehaviour,
    /// None: different kinds are allowed
    pub foreign_kind: Option<String>,
}

impl ForeignKey {
    pub fn new(
        path: impl Into<String>,
        parent_path: Option<impl Into<String>>,
        ty: ForeignKeyType,
        foreign_kind: Option<impl Into<String>>,
    ) -> Self {
        Self {
            path: path.into(),
            ty,
            behaviour: ForeignKeyBehaviour::Fill,
            foreign_kind: foreign_kind.map(|x| x.into()),
            parent_path: parent_path.map(|x| x.into()),
        }
    }
}

#[derive(Debug, sqlx::Type, Clone, Copy, PartialEq, Eq)]
#[sqlx(type_name = "foreign_key_type", rename_all = "PascalCase")]
pub enum ForeignKeyType {
    One,
    OneOptional,
    OneOrMany,
    NoneOrMany,
}

#[derive(Debug, sqlx::Type, Clone)]
#[sqlx(type_name = "foreign_key_behaviour", rename_all = "PascalCase")]
pub enum ForeignKeyBehaviour {
    Fill,
    Ignore,
}

// ── ResourceCache ─────────────────────────────────────────────────────────────

/// An in-memory mapping from every alias (and canonical kind name) to its
/// canonical kind name.  Used by the default trait methods to transparently
/// resolve aliases before hitting the backend.
#[derive(Default)]
pub struct ResourceCache(RwLock<HashMap<String, String>>);

impl ResourceCache {
    /// Register a kind together with all its aliases.
    /// The kind itself is also registered so exact-kind lookups always succeed.
    pub fn add(&self, kind: &str, aliases: &[String]) {
        let mut map = self.0.write().unwrap();
        map.insert(kind.to_string(), kind.to_string());
        for alias in aliases {
            map.insert(alias.clone(), kind.to_string());
        }
    }

    /// Resolve an alias (or exact kind) to the canonical kind name.
    /// Returns `None` if the alias is not registered.
    pub fn resolve(&self, alias: &str) -> Option<String> {
        self.0.read().unwrap().get(alias).cloned()
    }
}

// ── SchemaDefinition ──────────────────────────────────────────────────────────

/// A single schema registration — kind, api_version, aliases, FK constraints,
/// and the pre-computed JSON schema string.
pub struct SchemaDefinition {
    pub api_version: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub foreign_keys: Vec<ForeignKey>,
    pub json_schema: String,
}

impl SchemaDefinition {
    pub fn new<T: JsonSchema>(
        api_version: impl Into<String>,
        kind: impl Into<String>,
        aliases: impl IntoIterator<Item = impl Into<String>>,
        foreign_keys: impl IntoIterator<Item = ForeignKey>,
    ) -> Self {
        let schema = schemars::schema_for!(T);
        Self {
            api_version: api_version.into(),
            kind: kind.into(),
            aliases: aliases.into_iter().map(|x| x.into()).collect(),
            foreign_keys: foreign_keys.into_iter().collect(),
            json_schema: serde_json::to_string(&schema).unwrap(),
        }
    }
}

// ── NewDawnStoreBackend shared types ─────────────────────────────────────────

/// A single FK edge produced by the apply handler's FK graph walk.
///
/// Stored in the `relations` table so that `get` can fill navigation properties
/// (e.g. `parent_object`) without extra queries.
pub struct ObjectRelation {
    /// String ID of the object that declares the FK (`namespace/kind/name`).
    pub object_string_id: String,
    /// UUID of the FK constraint definition — used as the primary key in the
    /// `relations` table together with `object_string_id` + `target_string_id`.
    pub fk_constraint_id: Uuid,
    /// String ID of the FK-referenced target (`namespace/kind/name`).
    pub target_string_id: String,
}

// ── NewDawnStoreBackend raw data types ────────────────────────────────────────

/// A raw schema entry as returned by the backend — used to populate the schema cache.
#[derive(Clone)]
pub struct RawSchema {
    pub api_version: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub json_schema: String,
}

/// A raw foreign key constraint as returned by the backend — used to populate the FK cache.
#[derive(Clone)]
pub struct RawForeignKeyConstraint {
    pub id: Uuid,
    pub api_version: String,
    pub kind: String,
    pub key_path: String,
    pub ty: ForeignKeyType,
    pub behaviour: ForeignKeyBehaviour,
    pub foreign_key_kind: Option<String>,
    pub parent_key_path: Option<String>,
}

// ── NewDawnStoreBackend trait ─────────────────────────────────────────────────

/// Filter type used by [`NewDawnStoreBackend::get_objects`].
///
/// A focused alternative to [`GetObjectsFilter`] that only exposes the fields
/// needed by the cache initialisation routines and the get handler.
#[derive(Debug, Clone, Default)]
pub struct BackendGetObjectsFilter {
    pub namespace: Option<String>,
    pub kind: Option<String>,
    pub name: Option<String>,
    /// RBAC constraint injected by the get handler.
    /// `None` = unrestricted (superadmin / unauthenticated path).
    /// `Some([])` = deny all (caller has no matching Get grants).
    pub allowed: Option<Vec<AllowedScope>>,
    /// When `true`, the backend should populate `_object` navigation properties
    /// on returned objects by following their FK relations.
    pub fill_child_foreign_keys: bool,
}

/// Placeholder backend trait used during the cache-layer refactor.
///
/// Implementations provide the raw data required to populate [`crate::cache::DawnstoreCache`].
/// This trait will eventually replace [`DawnstoreBackend`].
pub trait NewDawnStoreBackend: Send + Sync {
    /// Return all registered schemas. Used by the schema cache initialiser.
    fn load_all_schemas(
        &self,
    ) -> impl Future<Output = Result<Vec<RawSchema>, DawnStoreError>> + Send;

    /// Return all registered foreign key constraints across all kinds.
    /// Used by the FK cache initialiser.
    fn load_all_foreign_key_constraints(
        &self,
    ) -> impl Future<Output = Result<Vec<RawForeignKeyConstraint>, DawnStoreError>> + Send;

    /// Return all objects matching `filter`. Used by the permission cache initialiser
    /// to load RBAC objects from the backend.
    fn get_objects(
        &self,
        filter: &BackendGetObjectsFilter,
    ) -> impl Future<Output = Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError>> + Send;

    /// Fetch a single object by its identity triple. Returns `None` if no object
    /// with that `(namespace, kind, name)` exists. Used by the apply handler's FK
    /// graph walk to verify existence of referenced objects.
    fn get_object(
        &self,
        namespace: &str,
        kind: &str,
        name: &str,
    ) -> impl Future<Output = Result<Option<ReturnObject<serde_json::Value>>, DawnStoreError>> + Send;

    /// Persist `objects` and reconcile their FK `relations` in a single transaction.
    ///
    /// For each object: if a record with the same string ID already exists, update
    /// it (preserving its UUID and `created_at`); otherwise insert a new record.
    /// After upserting objects, delete any existing relation rows for those objects
    /// that are absent from `relations`, then insert the new relation set.
    ///
    /// Returns the upserted objects as [`ReturnObject`]s.
    fn upsert_objects(
        &self,
        objects: Vec<ObjectAny>,
        relations: Vec<ObjectRelation>,
    ) -> impl Future<Output = Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError>> + Send;

    /// Delete the object identified by `(namespace, kind, name)`.
    ///
    /// Returns `Ok(())` whether or not the object existed (idempotent).
    fn delete_object(
        &self,
        namespace: &str,
        kind: &str,
        name: &str,
    ) -> impl Future<Output = Result<(), DawnStoreError>> + Send;

    /// Return all registered resource definitions (schemas).
    fn get_resource_definitions(
        &self,
    ) -> impl Future<Output = Result<Vec<ResourceDefinition>, DawnStoreError>> + Send;
}

// ── DawnstoreBackend trait ────────────────────────────────────────────────────

pub trait DawnstoreBackend: Send + Sync {
    // ── Required methods ──────────────────────────────────────────────────────

    /// Returns the alias cache for this backend instance.
    fn resource_cache(&self) -> &ResourceCache;

    /// Seed multiple schemas in a single transaction. Schemas that already
    /// exist (matched by api_version + kind) are skipped.
    /// Implementations must also call `self.resource_cache().add(kind, aliases)`
    /// for every newly seeded schema so that alias resolution stays up to date.
    fn seed_schema(
        &self,
        schemas: &[SchemaDefinition],
    ) -> impl Future<Output = Result<(), DawnStoreError>> + Send;

    fn apply_raw(
        &self,
        data: serde_json::Value,
    ) -> impl Future<Output = Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError>> + Send;

    fn get_impl(
        &self,
        filter: &GetObjectsFilter,
    ) -> impl Future<Output = Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError>> + Send;

    fn delete_impl(
        &self,
        delete: &DeleteObject,
    ) -> impl Future<Output = Result<(), DawnStoreError>> + Send;

    fn get_resource_definition(
        &self,
        filter: &GetResourceDefinitionFilter,
    ) -> impl Future<Output = Result<Vec<ResourceDefinition>, DawnStoreError>> + Send;

    fn get_object_infos_impl(
        &self,
        filter: &GetObjectInfosFilter,
    ) -> impl Future<Output = Result<ObjectInfos, DawnStoreError>> + Send;

    /// Extract all FK-referenced object string IDs (`ns/kind/name`) from `spec`
    /// for the given `(api_version, kind)`. Used by the controller to check that
    /// the caller has `Get` access to every referenced object before applying.
    fn get_fk_refs(
        &self,
        api_version: &str,
        kind: &str,
        spec: &serde_json::Value,
        namespace: &str,
    ) -> impl Future<Output = Result<Vec<String>, DawnStoreError>> + Send;

    // ── Default methods ───────────────────────────────────────────────────────

    /// Type-safe apply: serializes `Object<T>` and delegates to `apply_raw`.
    fn apply<T: serde::Serialize + Send>(
        &self,
        obj: Object<T>,
    ) -> impl Future<Output = Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError>> + Send {
        async move { self.apply_raw(serde_json::to_value(obj)?).await }
    }

    /// Resolve `kind_or_alias` through the resource cache.
    /// Falls back to the input unchanged when the alias is not registered.
    fn resolve_kind(&self, kind_or_alias: &str) -> String {
        self.resource_cache().resolve(kind_or_alias).unwrap_or_else(|| kind_or_alias.to_string())
    }

    /// Get objects, resolving the `kind` field through aliases first.
    fn get(
        &self,
        filter: &GetObjectsFilter,
    ) -> impl Future<Output = Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError>> + Send {
        async move {
            let kind = filter.kind.as_deref().map(|k| self.resolve_kind(k));
            self.get_impl(&GetObjectsFilter { kind, ..filter.clone() }).await
        }
    }

    /// Delete an object, resolving the `kind` field through aliases first.
    fn delete(
        &self,
        delete: &DeleteObject,
    ) -> impl Future<Output = Result<(), DawnStoreError>> + Send {
        async move {
            let kind = self.resolve_kind(&delete.kind);
            self.delete_impl(&DeleteObject { kind, ..delete.clone() }).await
        }
    }

    /// Get object infos, resolving the `kind` field through aliases first.
    fn get_object_infos(
        &self,
        filter: &GetObjectInfosFilter,
    ) -> impl Future<Output = Result<ObjectInfos, DawnStoreError>> + Send {
        async move {
            let kind = filter.kind.as_deref().map(|k| self.resolve_kind(k));
            self.get_object_infos_impl(&GetObjectInfosFilter { kind, ..filter.clone() }).await
        }
    }
}
