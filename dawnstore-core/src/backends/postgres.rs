use std::collections::{BTreeMap, HashMap};

use chrono::Utc;
use serde_json::Value;
use sqlx::{Pool, Postgres, migrate::MigrateError};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::{
    backends::postgres::data_models::{ForeignKeyConstraint, Object, ObjectSchema},
    error::DawnStoreError,
    models::{ForeignKey, ForeignKeyType},
};

use dawnstore_lib::*;

mod data_models;
mod queries;

pub struct PostgresBackend {
    pool: Pool<Postgres>,
    foreign_key_cache: RwLock<HashMap<ObjectId, Vec<ForeignKeyConstraint>>>,
    schema_cache: RwLock<HashMap<ObjectId, jsonschema::Validator>>,
}

impl PostgresBackend {
    pub fn new(pool: Pool<Postgres>) -> Self {
        PostgresBackend {
            pool,
            foreign_key_cache: Default::default(),
            schema_cache: Default::default(),
        }
    }

    pub async fn seed_object_schema<T: schemars::JsonSchema>(
        &self,
        api_version: impl Into<String>,
        kind: impl Into<String>,
        aliases: impl IntoIterator<Item = impl Into<String>>,
        foreign_keys: impl IntoIterator<Item = ForeignKey>,
    ) -> Result<(), DawnStoreError> {
        let api_version = api_version.into();
        let kind = kind.into();
        let mut trans = self.pool.begin().await?;
        let obj = queries::get_object_schema(trans.as_mut(), &api_version, &kind).await?;
        if obj.is_some() {
            return Ok(());
        }
        let schema = schemars::schema_for!(T);
        let schema = serde_json::to_string(&schema)?;
        queries::insert_object_schema(
            trans.as_mut(),
            &ObjectSchema {
                id: Uuid::new_v4(),
                api_version: api_version.clone(),
                kind: kind.clone(),
                json_schema: schema,
                aliases: aliases.into_iter().map(|x| x.into()).collect(),
            },
        )
        .await?;
        let foreign_keys = foreign_keys.into_iter();
        let mut keys = Vec::<ForeignKeyConstraint>::new();
        for key in foreign_keys {
            keys.push(ForeignKeyConstraint {
                id: Uuid::new_v4(),
                api_version: api_version.clone(),
                kind: kind.clone(),
                key_path: key.path,
                r#type: key.ty,
                behaviour: key.behaviour,
                foreign_key_kind: key.foreign_kind,
                parent_key_path: key.parent_path,
            });
        }
        queries::insert_multiple_foreign_key_constraints(trans.as_mut(), keys.as_slice()).await?;
        trans.commit().await?;

        Ok(())
    }

    pub async fn sqlx_migrate(&self) -> Result<(), MigrateError> {
        sqlx::migrate!("./migrations").run(&self.pool).await
    }

    pub async fn delete(&self, delete: &DeleteObject) -> Result<(), DawnStoreError> {
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

    pub async fn get(
        &self,
        filter: &GetObjectsFilter,
    ) -> Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError> {
        let objs = queries::get_objects_by_filter(&self.pool, filter).await?;
        Ok(objs
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
                // todo set the owners
                owners: Some(Default::default()),
                spec: x.spec.0,
            })
            .collect())
    }

    pub async fn get_resource_definition(
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

    pub async fn apply_raw(
        &self,
        mut data: serde_json::Value,
    ) -> Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError> {
        let mut input_objects = Vec::<ObjectAny>::new();
        // ingest objects and get raw objects
        if let Value::Array(_) = &data {
            input_objects = serde_json::from_value(data)?;
        } else if let Value::Object(x) = &mut data {
            if let Some(kind) = x.get("kind") {
                if kind == "List" {
                    if let Some(list) = x.remove("list") {
                        input_objects = serde_json::from_value(list)?;
                        if let Some(Value::String(object_kind)) = x.get("object_kind") {
                            input_objects
                                .iter_mut()
                                .for_each(|x| x.kind = Some(object_kind.clone()));
                        };
                        if let Some(Value::String(object_api_version)) = x.get("object_api_version")
                        {
                            input_objects
                                .iter_mut()
                                .for_each(|x| x.api_version = Some(object_api_version.clone()));
                        };
                    } else {
                        return Err(DawnStoreError::InvalidInputObjectMissingKindField);
                    }
                } else {
                    input_objects.push(serde_json::from_value(data)?);
                }
            } else {
                return Err(DawnStoreError::InvalidInputObjectMissingKindField);
            }
        } else {
            return Err(DawnStoreError::InvalidRootInputObject);
        }

        // validate if objects have all required fields and if the underlying schema is sound
        let mut schema_cache = self.schema_cache.read().await;
        let mut foreign_key_cache = self.foreign_key_cache.read().await;
        let mut string_ids = Vec::<String>::with_capacity(input_objects.len());
        let mut input_objects_with_string_id = Vec::<(String, ObjectAny)>::new();

        for obj in input_objects {
            let Some(api_version) = &obj.api_version else {
                return Err(DawnStoreError::ApiVersionMissingInObject);
            };
            let Some(kind) = &obj.kind else {
                return Err(DawnStoreError::KindMissingInObject);
            };
            let ns = obj.namespace.as_deref().unwrap_or("default");
            let object_id = ObjectId {
                kind: kind.clone(),
                api_version: api_version.clone(),
            };
            let validator = match schema_cache.get(&object_id) {
                Some(x) => x,
                None => {
                    drop(schema_cache);
                    let mut conn = self.pool.acquire().await?;
                    let Some(schema) =
                        queries::get_object_schema(&mut conn, api_version, kind).await?
                    else {
                        return Err(DawnStoreError::NoSchemaForObjectFound {
                            api_version: api_version.clone(),
                            kind: kind.clone(),
                        });
                    };
                    let validator =
                        jsonschema::validator_for(&serde_json::from_str(&schema.json_schema)?)?;
                    self.schema_cache
                        .write()
                        .await
                        .insert(object_id.clone(), validator);
                    schema_cache = self.schema_cache.read().await;
                    schema_cache
                        .get(&object_id)
                        .expect("we just added this thing")
                }
            };
            if let Err(e) = validator.validate(&obj.spec) {
                return Err(DawnStoreError::ObjectValidationError {
                    api_version: api_version.clone(),
                    kind: kind.clone(),
                    name: obj.name.clone(),
                    validation_error: e.to_owned(),
                });
            }
            let string_id = format!("{}/{}/{}", ns, kind, obj.name,);

            // check if the foreign keys are valid
            let foreign_keys = match foreign_key_cache.get(&object_id) {
                Some(x) => x,
                None => {
                    drop(foreign_key_cache);
                    let mut conn = self.pool.acquire().await?;
                    let costraints =
                        queries::get_foreign_key_constraints(&mut conn, api_version, kind).await?;
                    self.foreign_key_cache
                        .write()
                        .await
                        .insert(object_id.clone(), costraints);
                    foreign_key_cache = self.foreign_key_cache.read().await;
                    foreign_key_cache
                        .get(&object_id)
                        .expect("we just added the constraints")
                }
            };

            let mut fk_string_ids = Vec::<String>::new();
            'outer: for key in foreign_keys {
                let path_segments = key.key_path.split(".");
                let mut key_position = &Value::Null;
                for seg in path_segments {
                    key_position = match obj.spec.get(seg) {
                        Some(x) => x,
                        None if key.r#type == ForeignKeyType::OneOptional => continue 'outer,
                        None if key.r#type == ForeignKeyType::NoneOrMany => continue 'outer,
                        None => {
                            return Err(DawnStoreError::ObjectValidationMissingForeignKeyEntry {
                                api_version: api_version.clone(),
                                kind: kind.clone(),
                                name: obj.name.clone(),
                                foreign_key_path: key.key_path.clone(),
                                foreign_key_type: key.r#type.clone(),
                            });
                        }
                    };
                }
                let foreign_key_values = match (&key.r#type, key_position) {
                    (ForeignKeyType::One, Value::String(x)) => vec![x],
                    (ForeignKeyType::OneOptional, Value::Null) => vec![],
                    (ForeignKeyType::OneOptional, Value::String(x)) => vec![x],
                    (ForeignKeyType::OneOrMany, Value::String(x)) => vec![x],
                    (ForeignKeyType::OneOrMany, Value::Array(values)) => values
                        .iter()
                        .filter_map(|x| match x {
                            Value::String(x) => Some(x),
                            _ => None,
                        })
                        .collect(),
                    (ForeignKeyType::NoneOrMany, Value::Null) => vec![],
                    (ForeignKeyType::NoneOrMany, Value::String(x)) => vec![x],
                    (ForeignKeyType::NoneOrMany, Value::Array(values)) => values
                        .iter()
                        .filter_map(|x| match x {
                            Value::String(x) => Some(x),
                            _ => None,
                        })
                        .collect(),
                    _ => {
                        return Err(DawnStoreError::ObjectValidationMissingForeignKeyEntry {
                            api_version: api_version.clone(),
                            kind: kind.clone(),
                            name: obj.name.clone(),
                            foreign_key_path: key.key_path.clone(),
                            foreign_key_type: key.r#type.clone(),
                        });
                    }
                };
                for fk_val in foreign_key_values {
                    let comps = fk_val.split("/").collect::<Vec<_>>();
                    let (ns, fk_kind, fk_name) = match comps.as_slice() {
                        [ns, kind, name] => (*ns, *kind, *name),
                        // assume same ns as the current object
                        [kind, name] => (ns, *kind, *name),
                        // assume same ns and kind as the current object
                        [name] => (ns, kind.as_str(), *name),
                        _ => {
                            return Err(
                                DawnStoreError::ObjectValidationWrongForeignKeyEntryFormat {
                                    api_version: api_version.clone(),
                                    kind: kind.clone(),
                                    name: obj.name.clone(),
                                    foreign_key_path: key.key_path.clone(),
                                    foreign_key_type: key.r#type.clone(),
                                    value: fk_val.clone(),
                                },
                            );
                        }
                    };

                    if let Some(k) = &key.foreign_key_kind
                        && k.as_str() != fk_kind
                    {
                        return Err(DawnStoreError::ObjectValidationWrongForeignKeyEntryKind {
                            api_version: api_version.clone(),
                            kind: kind.clone(),
                            name: obj.name.clone(),
                            foreign_key_path: key.key_path.clone(),
                            foreign_key_type: key.r#type.clone(),
                            value: fk_val.clone(),
                        });
                    }

                    fk_string_ids.push(format!("{ns}/{fk_kind}/{fk_name}"));
                }
            }

            dbg!(&fk_string_ids);
            for fk in fk_string_ids {
                if string_ids.contains(&fk) {
                    continue;
                }

                if !queries::object_exists(&self.pool, fk.as_str()).await? {
                    return Err(DawnStoreError::ObjectValidationForeignKeyNotFound {
                        api_version: api_version.clone(),
                        kind: kind.clone(),
                        name: obj.name.clone(),
                        value: fk,
                    });
                }
            }

            string_ids.push(string_id.clone());
            input_objects_with_string_id.push((string_id, obj));
        }

        let mut database_objects = Vec::<Object>::with_capacity(input_objects_with_string_id.len());

        let mut trans = self.pool.begin().await?;
        let object_infos = queries::get_object_infos(trans.as_mut(), string_ids.as_slice()).await?;
        let object_infos = object_infos
            .iter()
            .map(|x| (&x.string_id, (&x.id, &x.created_at)))
            .collect::<BTreeMap<_, _>>();

        for (string_id, obj) in input_objects_with_string_id {
            let oi = object_infos.get(&string_id);
            let (id, created_at) = match &oi {
                Some((id, created_at)) => (**id, **created_at),
                None => (uuid::Uuid::new_v4(), Utc::now()),
            };
            let new_obj = Object {
                id,
                string_id,
                api_version: obj.api_version.unwrap(),
                name: obj.name,
                kind: obj.kind.unwrap(),
                created_at,
                updated_at: Utc::now(),
                namespace: obj.namespace.unwrap_or("default".to_string()),
                annotations: sqlx::types::Json(obj.annotations.unwrap_or_default()),
                labels: sqlx::types::Json(obj.labels.unwrap_or_default()),
                // todo add the owner references
                owners: Default::default(),
                spec: sqlx::types::Json(obj.spec),
            };
            database_objects.push(new_obj);
        }

        queries::insert_or_update_multiple_objects(trans.as_mut(), &database_objects).await?;
        trans.commit().await?;

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
                // todo set the owners
                owners: Some(Default::default()),
                spec: x.spec.0,
            })
            .collect())
    }
}
