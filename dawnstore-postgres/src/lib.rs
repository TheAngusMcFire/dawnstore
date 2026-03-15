use std::collections::{HashMap, HashSet};

use chrono::Utc;
use serde_json::Value;
use sqlx::{PgPool, Pool, Postgres, migrate::MigrateError};
use uuid::Uuid;

use data_models::{ForeignKeyConstraint, ObjectInfo, ObjectSchema, Relation};
use dawnstore_core::{abstractions::{
    BackendGetObjectsFilter, DawnstoreBackend, ForeignKey, ObjectRelation,
    RawForeignKeyConstraint, RawSchema, SchemaDefinition,
}, error::DawnStoreError};
use dawnstore_core::rbac::helpers::object_string_id;
use dawnstore_lib::*;

mod data_models;
mod queries;

pub struct PostgresBackend {
    pool: Pool<Postgres>,
}

// ── Postgres-specific methods ─────────────────────────────────────────────────

impl PostgresBackend {
    pub fn get_pool(&self) -> &Pool<Postgres> {
        &self.pool
    }

    pub async fn new_from_connection_string(
        connection_string: impl Into<String>,
    ) -> Result<Self, DawnStoreError> {
        let pool = PgPool::connect(&connection_string.into()).await?;
        Ok(Self::new(pool))
    }

    pub fn new(pool: Pool<Postgres>) -> Self {
        PostgresBackend { pool }
    }

    /// Convenience wrapper for seeding a single schema from a Rust type.
    pub async fn seed_object_schema<T: schemars::JsonSchema>(
        &self,
        api_version: impl Into<String>,
        kind: impl Into<String>,
        aliases: impl IntoIterator<Item = impl Into<String>>,
        foreign_keys: impl IntoIterator<Item = ForeignKey>,
    ) -> Result<(), DawnStoreError> {
        let def = SchemaDefinition::new::<T>(api_version, kind, aliases, foreign_keys);
        self.seed_schemas(&[def]).await
    }

    pub async fn sqlx_migrate(&self) -> Result<(), MigrateError> {
        sqlx::migrate!("./migrations").run(&self.pool).await
    }
}

// ── DawnstoreBackend trait impl ───────────────────────────────────────────────

impl DawnstoreBackend for PostgresBackend {
    fn load_all_schemas(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<RawSchema>, DawnStoreError>> + Send {
        let pool = self.pool.clone();
        async move {
            let schemas = queries::get_all_object_schemas(&pool).await?;
            Ok(schemas
                .into_iter()
                .map(|s| RawSchema {
                    api_version: s.api_version,
                    kind: s.kind,
                    aliases: s.aliases,
                    json_schema: s.json_schema,
                })
                .collect())
        }
    }

    fn load_all_foreign_key_constraints(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<RawForeignKeyConstraint>, DawnStoreError>> + Send
    {
        let pool = self.pool.clone();
        async move {
            let fks = queries::get_all_foreign_key_constraints(&pool).await?;
            Ok(fks
                .into_iter()
                .map(|f| RawForeignKeyConstraint {
                    id: f.id,
                    api_version: f.api_version,
                    kind: f.kind,
                    key_path: f.key_path,
                    ty: f.r#type,
                    behaviour: f.behaviour,
                    foreign_key_kind: f.foreign_key_kind,
                    parent_key_path: f.parent_key_path,
                })
                .collect())
        }
    }

    fn get_objects(
        &self,
        filter: &BackendGetObjectsFilter,
    ) -> impl std::future::Future<Output = Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError>> + Send
    {
        let filter = filter.clone();
        async move { self.get_objects_impl(&filter).await }
    }

    fn get_object(
        &self,
        namespace: &str,
        kind: &str,
        name: &str,
    ) -> impl std::future::Future<Output = Result<Option<ReturnObject<serde_json::Value>>, DawnStoreError>> + Send
    {
        let filter = BackendGetObjectsFilter {
            namespace: Some(namespace.to_string()),
            kind: Some(kind.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        };
        async move {
            let results = self.get_objects_impl(&filter).await?;
            Ok(results.into_iter().next())
        }
    }

    fn upsert_objects(
        &self,
        objects: Vec<ObjectAny>,
        relations: Vec<ObjectRelation>,
    ) -> impl std::future::Future<Output = Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError>> + Send
    {
        let pool = self.pool.clone();
        async move {
            // Collect all string IDs we need to resolve (objects + FK targets).
            let mut all_string_ids: HashSet<String> = HashSet::new();
            let mut objects_with_sid: Vec<(String, ObjectAny)> =
                Vec::with_capacity(objects.len());

            for obj in objects {
                let ns = obj.namespace.as_deref().unwrap_or("default");
                let kind = obj.kind.as_deref().ok_or(DawnStoreError::KindMissingInObject)?;
                let sid = object_string_id(ns, kind, &obj.name);
                all_string_ids.insert(sid.clone());
                objects_with_sid.push((sid, obj));
            }
            for rel in &relations {
                all_string_ids.insert(rel.object_string_id.clone());
                all_string_ids.insert(rel.target_string_id.clone());
            }
            let all_string_ids_vec: Vec<String> = all_string_ids.into_iter().collect();

            let mut con = pool.begin().await?;

            // Look up existing DB records so we can preserve UUIDs + created_at.
            let mut object_infos: HashMap<String, ObjectInfo> =
                queries::get_object_infos(con.as_mut(), &all_string_ids_vec)
                    .await?
                    .into_iter()
                    .map(|x| (x.string_id.clone(), x))
                    .collect();

            // Build Object records, preserving existing UUIDs on update.
            let now = Utc::now();
            let mut database_objects: Vec<data_models::Object> =
                Vec::with_capacity(objects_with_sid.len());
            for (sid, obj) in objects_with_sid {
                let oi = object_infos.get(&sid);
                let (id, created_at) = match oi {
                    Some(oi) => (oi.id, oi.created_at),
                    None => (Uuid::new_v4(), now),
                };
                database_objects.push(data_models::Object {
                    id,
                    string_id: sid,
                    api_version: obj
                        .api_version
                        .ok_or(DawnStoreError::ApiVersionMissingInObject)?,
                    name: obj.name,
                    kind: obj.kind.ok_or(DawnStoreError::KindMissingInObject)?,
                    created_at,
                    updated_at: now,
                    namespace: obj.namespace.unwrap_or_else(|| "default".to_string()),
                    annotations: sqlx::types::Json(obj.annotations.unwrap_or_default()),
                    labels: sqlx::types::Json(obj.labels.unwrap_or_default()),
                    spec: sqlx::types::Json(obj.spec),
                });
            }

            queries::insert_or_update_multiple_objects(con.as_mut(), &database_objects).await?;

            // Update the object_infos map with newly inserted UUIDs.
            for db_obj in &database_objects {
                object_infos.entry(db_obj.string_id.clone()).or_insert_with(|| ObjectInfo {
                    id: db_obj.id,
                    string_id: db_obj.string_id.clone(),
                    created_at: db_obj.created_at,
                });
            }

            // IDs of the objects we just upserted (for relation reconciliation).
            let object_db_ids: Vec<Uuid> = database_objects.iter().map(|x| x.id).collect();

            // Build Relation records by resolving string IDs to UUIDs.
            let mut new_relations: Vec<Relation> = Vec::with_capacity(relations.len());
            for rel in &relations {
                let oi = object_infos
                    .get(&rel.object_string_id)
                    .ok_or_else(|| DawnStoreError::ForeignKeyNotFound(rel.object_string_id.clone()))?;
                let foi = object_infos
                    .get(&rel.target_string_id)
                    .ok_or_else(|| DawnStoreError::ForeignKeyNotFound(rel.target_string_id.clone()))?;
                new_relations.push(Relation {
                    object_id: oi.id,
                    foreign_object_id: foi.id,
                    foreign_key_id: rel.fk_constraint_id,
                });
            }

            // Reconcile: fetch existing relations, delete stale ones, insert new ones.
            let existing_relations =
                queries::get_relations_of_objects(con.as_mut(), &object_db_ids).await?;
            let relations_to_delete: Vec<Relation> = existing_relations
                .into_iter()
                .filter(|x| {
                    !new_relations.iter().any(|y| {
                        y.object_id == x.object_id
                            && y.foreign_object_id == x.foreign_object_id
                            && y.foreign_key_id == x.foreign_key_id
                    })
                })
                .collect();

            queries::delete_multiple_relations(
                con.as_mut(),
                &relations_to_delete.iter().map(|x| x.object_id).collect::<Vec<_>>(),
                &relations_to_delete.iter().map(|x| x.foreign_object_id).collect::<Vec<_>>(),
                &relations_to_delete.iter().map(|x| x.foreign_key_id).collect::<Vec<_>>(),
            )
            .await?;
            queries::insert_multiple_relation(con.as_mut(), &new_relations).await?;
            con.commit().await?;

            Ok(database_objects
                .into_iter()
                .map(|x| ReturnObject {
                    id: x.id,
                    namespace: x.namespace,
                    api_version: x.api_version,
                    kind: x.kind,
                    name: x.name,
                    created_at: x.created_at,
                    updated_at: x.updated_at,
                    annotations: Some(x.annotations.0),
                    labels: Some(x.labels.0),
                    spec: x.spec.0,
                })
                .collect())
        }
    }

    fn delete_object(
        &self,
        namespace: &str,
        kind: &str,
        name: &str,
    ) -> impl std::future::Future<Output = Result<(), DawnStoreError>> + Send {
        let pool = self.pool.clone();
        let ns = if namespace == "default" { None } else { Some(namespace.to_string()) };
        let kind = kind.to_string();
        let name = name.to_string();
        async move {
            let mut con = pool.acquire().await?;
            queries::delete_object(con.as_mut(), ns.as_deref(), &name, &kind).await?;
            Ok(())
        }
    }

    fn get_resource_definitions(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<ResourceDefinition>, DawnStoreError>> + Send
    {
        let pool = self.pool.clone();
        async move {
            let schemas = queries::get_all_object_schemas(&pool).await?;
            Ok(schemas
                .into_iter()
                .map(|s| ResourceDefinition {
                    api_version: s.api_version,
                    kind: s.kind,
                    aliases: s.aliases,
                    json_schema: s.json_schema,
                })
                .collect())
        }
    }

    fn get_inbound_references(
        &self,
        namespace: &str,
        kind: &str,
        name: &str,
    ) -> impl std::future::Future<Output = Result<Vec<String>, DawnStoreError>> + Send {
        let pool = self.pool.clone();
        let string_id = object_string_id(namespace, kind, name);
        async move {
            let mut con = pool.acquire().await?;
            Ok(queries::get_objects_referencing(con.as_mut(), &string_id).await?)
        }
    }

    fn seed_schemas(
        &self,
        schemas: &[SchemaDefinition],
    ) -> impl std::future::Future<Output = Result<(), DawnStoreError>> + Send {
        // Collect schema data to avoid lifetime issues across the await points.
        let schema_data: Vec<(String, String, String, Vec<String>, Vec<ForeignKeyConstraint>)> =
            schemas
                .iter()
                .map(|def| {
                    let keys: Vec<ForeignKeyConstraint> = def
                        .foreign_keys
                        .iter()
                        .map(|key| ForeignKeyConstraint {
                            id: Uuid::new_v4(),
                            api_version: def.api_version.clone(),
                            kind: def.kind.clone(),
                            key_path: key.path.clone(),
                            r#type: key.ty.clone(),
                            behaviour: key.behaviour.clone(),
                            foreign_key_kind: key.foreign_kind.clone(),
                            parent_key_path: key.parent_path.clone(),
                        })
                        .collect();
                    (
                        def.api_version.clone(),
                        def.kind.clone(),
                        def.json_schema.clone(),
                        def.aliases.clone(),
                        keys,
                    )
                })
                .collect();
        let pool = self.pool.clone();
        async move {
            let mut trans = pool.begin().await?;
            for (api_version, kind, json_schema, aliases, keys) in schema_data {
                if queries::get_object_schema(trans.as_mut(), &api_version, &kind)
                    .await?
                    .is_some()
                {
                    continue;
                }
                queries::insert_object_schema(
                    trans.as_mut(),
                    &ObjectSchema {
                        id: Uuid::new_v4(),
                        api_version: api_version.clone(),
                        kind: kind.clone(),
                        json_schema,
                        aliases,
                    },
                )
                .await?;
                queries::insert_multiple_foreign_key_constraints(trans.as_mut(), &keys).await?;
            }
            trans.commit().await?;
            Ok(())
        }
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

impl PostgresBackend {
    async fn get_objects_impl(
        &self,
        filter: &BackendGetObjectsFilter,
    ) -> Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError> {
        let mut con = self.pool.acquire().await?;
        let objs = queries::get_objects_by_filter(con.as_mut(), filter).await?;

        let obj_ids = objs.iter().map(|x| x.id).collect::<Vec<_>>();
        let relations = queries::get_relations_of_objects(con.as_mut(), obj_ids.as_slice()).await?;
        let foreign_object_ids = relations.iter().map(|x| x.foreign_object_id).collect::<Vec<_>>();

        let foreign_objects: Vec<ReturnAny> =
            queries::get_objects(con.as_mut(), foreign_object_ids.as_slice())
                .await?
                .into_iter()
                .map(|x| ReturnAny {
                    id: x.id,
                    namespace: x.namespace,
                    api_version: x.api_version,
                    kind: x.kind,
                    name: x.name,
                    created_at: x.created_at,
                    updated_at: x.updated_at,
                    annotations: Some(x.annotations.0),
                    labels: Some(x.labels.0),
                    spec: x.spec.0,
                })
                .collect();

        let objects: Vec<ReturnAny> = objs
            .into_iter()
            .map(|x| ReturnAny {
                id: x.id,
                namespace: x.namespace,
                api_version: x.api_version,
                kind: x.kind,
                name: x.name,
                created_at: x.created_at,
                updated_at: x.updated_at,
                annotations: Some(x.annotations.0),
                labels: Some(x.labels.0),
                spec: x.spec.0,
            })
            .collect();

        if !filter.fill_child_foreign_keys {
            return Ok(objects);
        }

        let mut objects = objects;
        for obj in &mut objects {
            let fk_constraints_raw =
                queries::get_foreign_key_constraints(con.as_mut(), &obj.api_version, &obj.kind)
                    .await?;

            for fkc in fk_constraints_raw.iter() {
                let fk_ids = relations
                    .iter()
                    .filter(|x| x.foreign_key_id == fkc.id)
                    .collect::<Vec<_>>();

                let mut objs = foreign_objects
                    .iter()
                    .filter(|o| fk_ids.iter().any(|x| x.foreign_object_id == o.id))
                    .collect::<Vec<_>>();

                use dawnstore_core::abstractions::ForeignKeyType;
                let obj_path = match fkc.r#type {
                    ForeignKeyType::OneOrMany | ForeignKeyType::NoneOrMany => {
                        format!("{}_objects", fkc.key_path)
                    }
                    ForeignKeyType::One | ForeignKeyType::OneOptional => {
                        format!("{}_object", fkc.key_path)
                    }
                };
                let mut path_segments = obj_path.split(".").collect::<Vec<_>>();
                let last_segment = path_segments.pop();
                let mut key_position = &mut obj.spec;
                for seg in path_segments {
                    if key_position.get(seg).is_none() {
                        if let Value::Object(x) = key_position {
                            x.insert(seg.to_string(), Value::Object(Default::default()));
                        } else {
                            return Err(DawnStoreError::InternalServerError(
                                "unexpected json value of field".to_string(),
                            ));
                        }
                    }
                    key_position = key_position.get_mut(seg).unwrap();
                }

                if let (Some(seg), Value::Object(x)) = (last_segment, key_position) {
                    let value = match fkc.r#type {
                        ForeignKeyType::One | ForeignKeyType::OneOptional => serde_json::to_value(objs.pop())?,
                        ForeignKeyType::OneOrMany | ForeignKeyType::NoneOrMany => serde_json::to_value(objs)?,
                    };
                    x.insert(seg.to_string(), value);
                }
            }
        }

        Ok(objects)
    }
}
