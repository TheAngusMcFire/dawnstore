use std::future::Future;

pub use dawnstore_lib::*;
use schemars::JsonSchema;

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
        Self {
            api_version: api_version.into(),
            kind: kind.into(),
            aliases: aliases.into_iter().map(|x| x.into()).collect(),
            foreign_keys: foreign_keys.into_iter().collect(),
            json_schema: serde_json::to_string(&schema).unwrap(),
        }
    }
}

// ── DawnstoreBackend trait ────────────────────────────────────────────────────

pub trait DawnstoreBackend: Send + Sync {
    /// Seed multiple schemas in a single transaction. Schemas that already
    /// exist (matched by api_version + kind) are skipped.
    fn seed_schema(
        &self,
        schemas: &[SchemaDefinition],
    ) -> impl Future<Output = Result<(), DawnStoreError>> + Send;

    /// Type-safe apply: serializes `Object<T>` and delegates to `apply_raw`.
    fn apply<T: serde::Serialize + Send>(
        &self,
        obj: Object<T>,
    ) -> impl Future<Output = Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError>> + Send {
        async move { self.apply_raw(serde_json::to_value(obj)?).await }
    }

    fn apply_raw(
        &self,
        data: serde_json::Value,
    ) -> impl Future<Output = Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError>> + Send;

    fn get(
        &self,
        filter: &GetObjectsFilter,
    ) -> impl Future<Output = Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError>> + Send;

    fn delete(
        &self,
        delete: &DeleteObject,
    ) -> impl Future<Output = Result<(), DawnStoreError>> + Send;

    fn get_resource_definition(
        &self,
        filter: &GetResourceDefinitionFilter,
    ) -> impl Future<Output = Result<Vec<ResourceDefinition>, DawnStoreError>> + Send;

    fn get_object_infos(
        &self,
        filter: &GetObjectInfosFilter,
    ) -> impl Future<Output = Result<ObjectInfos, DawnStoreError>> + Send;
}
