use std::collections::{HashMap, HashSet};

use serde_json::Value;
use sqlx::{PgPool, Pool, Postgres, migrate::MigrateError};
use uuid::Uuid;

use cache::CacheStore;
use data_models::{ForeignKeyConstraint, ObjectInfo, ObjectSchema, Relation};
use dawnstore_core::{abstractions::{DawnstoreBackend, ForeignKey, ResourceCache}, error::DawnStoreError};
use dawnstore_lib::*;

mod apply_impl;
mod cache;
mod data_models;
mod queries;

pub struct PostgresBackend {
    pool: Pool<Postgres>,
    cache: CacheStore,
    resource_cache: ResourceCache,
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
        PostgresBackend {
            pool,
            cache: CacheStore::default(),
            resource_cache: ResourceCache::default(),
        }
    }

    /// Convenience wrapper for seeding a single schema from a Rust type.
    pub async fn seed_object_schema<T: schemars::JsonSchema>(
        &self,
        api_version: impl Into<String>,
        kind: impl Into<String>,
        aliases: impl IntoIterator<Item = impl Into<String>>,
        foreign_keys: impl IntoIterator<Item = ForeignKey>,
    ) -> Result<(), DawnStoreError> {
        use dawnstore_core::abstractions::SchemaDefinition;
        let def = SchemaDefinition::new::<T>(api_version, kind, aliases, foreign_keys);
        self.seed_schema(&[def]).await
    }

    pub async fn sqlx_migrate(&self) -> Result<(), MigrateError> {
        sqlx::migrate!("./migrations").run(&self.pool).await
    }

    pub async fn warm_caches(&self) -> Result<(), DawnStoreError> {
        self.cache.warm(&self.pool).await?;
        let schemas = queries::get_all_object_schemas(&self.pool).await?;
        for s in &schemas {
            self.resource_cache.add(&s.kind, &s.aliases);
        }
        Ok(())
    }
}

// ── DawnstoreBackend trait impl ───────────────────────────────────────────────

impl DawnstoreBackend for PostgresBackend {
    async fn seed_schema(
        &self,
        schemas: &[dawnstore_core::abstractions::SchemaDefinition],
    ) -> Result<(), DawnStoreError> {
        // Collect the schemas that aren't registered yet, then insert them all
        // in a single transaction.
        let mut trans = self.pool.begin().await?;
        let mut new_schemas: Vec<(&dawnstore_core::abstractions::SchemaDefinition, Vec<ForeignKeyConstraint>)> = Vec::new();

        for def in schemas {
            if queries::get_object_schema(trans.as_mut(), &def.api_version, &def.kind).await?.is_some() {
                continue;
            }
            queries::insert_object_schema(
                trans.as_mut(),
                &ObjectSchema {
                    id: uuid::Uuid::new_v4(),
                    api_version: def.api_version.clone(),
                    kind: def.kind.clone(),
                    json_schema: def.json_schema.clone(),
                    aliases: def.aliases.clone(),
                },
            )
            .await?;
            let keys: Vec<ForeignKeyConstraint> = def.foreign_keys.iter().map(|key| ForeignKeyConstraint {
                id: uuid::Uuid::new_v4(),
                api_version: def.api_version.clone(),
                kind: def.kind.clone(),
                key_path: key.path.clone(),
                r#type: key.ty.clone(),
                behaviour: key.behaviour.clone(),
                foreign_key_kind: key.foreign_kind.clone(),
                parent_key_path: key.parent_path.clone(),
            }).collect();
            queries::insert_multiple_foreign_key_constraints(trans.as_mut(), &keys).await?;
            new_schemas.push((def, keys));
        }
        trans.commit().await?;

        for (def, keys) in new_schemas {
            let validator = jsonschema::validator_for(&serde_json::from_str(&def.json_schema)?)?;
            self.cache.insert_schema(&def.api_version, &def.kind, validator).await;
            self.cache.insert_foreign_keys(&def.api_version, &def.kind, keys).await;
            self.resource_cache.add(&def.kind, &def.aliases);
        }

        Ok(())
    }

    fn resource_cache(&self) -> &ResourceCache {
        &self.resource_cache
    }

    async fn delete_impl(&self, delete: &DeleteObject) -> Result<(), DawnStoreError> {
        let mut con = self.pool.acquire().await?;
        let ns = match &delete.namespace {
            Some(x) if x == "default" => None,
            Some(x) => Some(x),
            None => None,
        }
        .map(|x| x.as_str());
        queries::delete_object(&mut con, ns, &delete.name, &delete.kind).await?;
        Ok(())
    }

    async fn get_impl(
        &self,
        filter: &GetObjectsFilter,
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

        let mut objects: Vec<ReturnAny> = objs
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

        for obj in &mut objects {
            let fk_constraints = self
                .cache
                .get_foreign_keys(con.as_mut(), &obj.api_version, &obj.kind)
                .await?;

            for fkc in fk_constraints.iter() {
                let fk_ids = relations
                    .iter()
                    .filter(|x| x.foreign_key_id == fkc.id)
                    .collect::<Vec<_>>();

                let mut objs = foreign_objects
                    .iter()
                    .filter(|o| fk_ids.iter().any(|x| x.foreign_object_id == o.id))
                    .collect::<Vec<_>>();

                let obj_path = format!("{}_object", fkc.key_path);
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
                    use dawnstore_core::abstractions::ForeignKeyType;
                    let value = match fkc.r#type {
                        ForeignKeyType::One | ForeignKeyType::OneOptional => serde_json::to_value(objs.pop())?,
                        ForeignKeyType::OneOrMany => serde_json::to_value(objs)?,
                        ForeignKeyType::NoneOrMany => serde_json::to_value(objs.pop())?,
                    };
                    x.insert(seg.to_string(), value);
                }
            }
        }

        Ok(objects)
    }

    async fn get_resource_definition(
        &self,
        _filter: &GetResourceDefinitionFilter,
    ) -> Result<Vec<ResourceDefinition>, DawnStoreError> {
        let objs = queries::get_all_object_schemas(&self.pool)
            .await?
            .into_iter()
            .map(|x| ResourceDefinition {
                api_version: x.api_version,
                kind: x.kind,
                aliases: x.aliases,
                json_schema: x.json_schema,
            })
            .collect();
        Ok(objs)
    }

    async fn apply_raw(
        &self,
        data: serde_json::Value,
    ) -> Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError> {
        let input_objects = apply_impl::build_base_objects_from_raw_value(data)?;

        let mut string_ids = Vec::<String>::with_capacity(input_objects.len());
        let mut input_objects_with_string_id = Vec::<(String, ObjectAny)>::new();
        let mut all_fks = HashMap::<String, Vec<(Vec<String>, Uuid)>>::default();

        for obj in input_objects {
            let Some(api_version) = &obj.api_version else {
                return Err(DawnStoreError::ApiVersionMissingInObject);
            };
            let Some(kind) = &obj.kind else {
                return Err(DawnStoreError::KindMissingInObject);
            };
            let ns = obj.namespace.as_deref().unwrap_or("default");
            let string_id = format!("{}/{}/{}", ns, kind, obj.name);

            let mut con = self.pool.acquire().await?;
            apply_impl::validate_object_schema(con.as_mut(), &self.cache, &obj, api_version, kind)
                .await?;

            let fks = apply_impl::check_foreign_keys(
                con.as_mut(),
                &self.cache,
                &obj,
                api_version,
                kind,
                ns,
            )
            .await?;

            string_ids.push(string_id.clone());
            input_objects_with_string_id.push((string_id.clone(), obj));
            all_fks.insert(string_id, fks);
        }

        let mut all_string_ids = HashSet::<&str>::new();
        string_ids.iter().for_each(|x| { all_string_ids.insert(x.as_str()); });
        all_fks.values().for_each(|x| {
            x.iter().for_each(|(ids, _)| {
                ids.iter().for_each(|x| { all_string_ids.insert(x.as_str()); });
            });
        });
        let all_string_ids = all_string_ids.into_iter().map(|x| x.to_owned()).collect::<Vec<_>>();

        let mut con = self.pool.begin().await?;
        let mut object_infos = queries::get_object_infos(con.as_mut(), all_string_ids.as_slice())
            .await?
            .into_iter()
            .map(|x| (x.string_id.clone(), x))
            .collect::<HashMap<String, ObjectInfo>>();
        let all_object_db_ids = all_fks
            .keys()
            .filter_map(|x| object_infos.get(x).map(|x| x.id))
            .collect::<Vec<_>>();
        let database_objects =
            apply_impl::maintain_objects(con.as_mut(), &object_infos, input_objects_with_string_id)
                .await?;
        database_objects.iter().for_each(|x| {
            let string_id = format!("{}/{}/{}", x.namespace, x.kind, x.name);
            object_infos.insert(string_id.clone(), ObjectInfo { id: x.id, string_id, created_at: x.created_at });
        });

        let mut foreign_key_objects = Vec::<Relation>::new();
        for (object_id, fks) in &all_fks {
            let Some(oi) = object_infos.get(object_id) else {
                return Err(DawnStoreError::ForeignKeyNotFound(object_id.clone()));
            };
            for (string_ids, fk_id) in fks {
                for sid in string_ids {
                    let Some(foi) = object_infos.get(sid) else {
                        return Err(DawnStoreError::ForeignKeyNotFound(sid.clone()));
                    };
                    foreign_key_objects.push(Relation {
                        object_id: oi.id,
                        foreign_object_id: foi.id,
                        foreign_key_id: *fk_id,
                    });
                }
            }
        }

        let existing_relations =
            queries::get_relations_of_objects(con.as_mut(), all_object_db_ids.as_slice()).await?;

        let relations_to_delete = existing_relations
            .into_iter()
            .filter(|x| {
                !foreign_key_objects.iter().any(|y| {
                    y.object_id == x.object_id
                        && y.foreign_object_id == x.foreign_object_id
                        && y.foreign_key_id == x.foreign_key_id
                })
            })
            .collect::<Vec<_>>();

        queries::delete_multiple_relations(
            con.as_mut(),
            relations_to_delete.iter().map(|x| x.object_id).collect::<Vec<_>>().as_slice(),
            relations_to_delete.iter().map(|x| x.foreign_object_id).collect::<Vec<_>>().as_slice(),
            relations_to_delete.iter().map(|x| x.foreign_key_id).collect::<Vec<_>>().as_slice(),
        )
        .await?;
        queries::insert_multiple_relation(con.as_mut(), foreign_key_objects.as_slice()).await?;
        con.commit().await?;

        Ok(database_objects
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
            .collect())
    }

    async fn get_object_infos_impl(
        &self,
        filter: &GetObjectInfosFilter,
    ) -> Result<ObjectInfos, DawnStoreError> {
        let mut con = self.pool.acquire().await?;
        let objs = queries::get_api_object_infos_with_filter(con.as_mut(), filter)
            .await?
            .into_iter()
            .map(|x| dawnstore_lib::ObjectInfo {
                namespace: x.namespace,
                id: x.id,
                api_version: x.api_version,
                kind: x.kind,
                name: x.name,
            })
            .collect();
        Ok(ObjectInfos { infos: objs })
    }
}
