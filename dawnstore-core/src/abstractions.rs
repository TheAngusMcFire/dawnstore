use std::future::Future;

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
        // Serialise to Value so we can inject fields from Object<T> that are
        // valid top-level fields in any apply/get request but live on the wrapper
        // type rather than the spec struct.  We use `entry(...).or_insert` so we
        // don't overwrite a field that the spec struct already declares itself.
        let mut schema_value = serde_json::to_value(&schema).unwrap();
        let nullable_string_map = serde_json::json!({
            "type": ["object", "null"],
            "additionalProperties": { "type": "string" }
        });
        if let Some(props) = schema_value
            .get_mut("properties")
            .and_then(|p| p.as_object_mut())
        {
            props
                .entry("labels")
                .or_insert_with(|| nullable_string_map.clone());
            props
                .entry("annotations")
                .or_insert_with(|| nullable_string_map);
        }
        Self {
            api_version: api_version.into(),
            kind: kind.into(),
            aliases: aliases.into_iter().map(|x| x.into()).collect(),
            foreign_keys: foreign_keys.into_iter().collect(),
            json_schema: serde_json::to_string(&schema_value).unwrap(),
        }
    }
}

// ── DawnstoreBackend shared types ─────────────────────────────────────────

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

// ── DawnstoreBackend raw data types ────────────────────────────────────────

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

// ── DawnstoreBackend trait ─────────────────────────────────────────────────

/// Filter type used by [`DawnstoreBackend::get_objects`].
///
/// A focused alternative to [`GetObjectsFilter`] that only exposes the fields
/// needed by the cache initialisation routines and the get handler.
#[derive(Debug, Clone, Default)]
pub struct BackendGetObjectsFilter {
    pub namespace: Option<String>,
    pub kind: Option<String>,
    pub name: Option<String>,
    pub ids: Option<Vec<Uuid>>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
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
pub trait DawnstoreBackend: Send + Sync {
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

    /// Return the string IDs (`namespace/kind/name`) of all objects that hold an
    /// inbound FK relation pointing at `(namespace, kind, name)`.
    ///
    /// Used by the delete handler to block deletes when referencing objects exist.
    fn get_inbound_references(
        &self,
        namespace: &str,
        kind: &str,
        name: &str,
    ) -> impl Future<Output = Result<Vec<String>, DawnStoreError>> + Send;

    /// Persist schema registrations (kind, api_version, aliases, FK constraints).
    /// Schemas that already exist are skipped (idempotent).
    fn seed_schemas(
        &self,
        schemas: &[SchemaDefinition],
    ) -> impl Future<Output = Result<(), DawnStoreError>> + Send;

    /// Return the string IDs (`namespace/kind/name`) of objects in OTHER namespaces
    /// that hold an inbound FK relation pointing at any object inside `namespace`.
    ///
    /// Used by the delete handler to block namespace deletions that would leave
    /// cross-namespace FK references dangling.
    fn get_cross_namespace_inbound_references(
        &self,
        namespace: &str,
    ) -> impl Future<Output = Result<Vec<String>, DawnStoreError>> + Send;

    /// Delete all objects whose `namespace` field equals `namespace`.
    ///
    /// The `relations` table rows are removed automatically via `ON DELETE CASCADE`.
    /// Does not delete the `Namespace` object itself (in the `system` namespace).
    fn delete_objects_by_namespace(
        &self,
        namespace: &str,
    ) -> impl Future<Output = Result<(), DawnStoreError>> + Send;
}
