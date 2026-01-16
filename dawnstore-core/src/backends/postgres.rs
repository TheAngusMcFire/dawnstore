use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use serde_json::Value;
use sqlx::{Pool, Postgres, migrate::MigrateError};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::{
    backends::postgres::data_models::{Object, ObjectSchema},
    error::DawnStoreError,
    models::{ObjectAny, ObjectId, ReturnAny, ReturnObject},
};

mod data_models;
mod queries;

pub struct PostgresBackend {
    pool: Pool<Postgres>,
    foraign_key_cache: RwLock<BTreeMap<(String, String), Vec<String>>>,
    schema_cache: RwLock<BTreeMap<ObjectId, jsonschema::Validator>>,
}

impl PostgresBackend {
    pub fn new(pool: Pool<Postgres>) -> Self {
        PostgresBackend {
            pool,
            foraign_key_cache: Default::default(),
            schema_cache: Default::default(),
        }
    }
    pub async fn seed_object_schema<T: schemars::JsonSchema>(
        &self,
        api_version: impl Into<String>,
        kind: impl Into<String>,
    ) -> Result<(), DawnStoreError> {
        let api_version = api_version.into();
        let kind = kind.into();
        let obj = queries::get_object_schema(&self.pool, &api_version, &kind).await?;
        if obj.is_some() {
            return Ok(());
        }
        let schema = schemars::schema_for!(T);
        let schema = serde_json::to_string(&schema)?;
        queries::insert_object_schema(
            &self.pool,
            &ObjectSchema {
                id: Uuid::new_v4(),
                api_version,
                kind,
                json_schema: schema,
            },
        )
        .await?;
        Ok(())
    }

    pub async fn sqlx_migrate(&self) -> Result<(), MigrateError> {
        sqlx::migrate!("./migrations").run(&self.pool).await
    }

    pub async fn apply_raw(
        &self,
        mut data: serde_json::Value,
    ) -> Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError> {
        let mut input_objects = Vec::<ObjectAny>::new();
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

        let mut schema_cache = self.schema_cache.read().await;
        let mut string_ids = Vec::<String>::with_capacity(input_objects.len());
        let mut input_objects_with_string_id = Vec::<(String, ObjectAny)>::new();
        for obj in input_objects {
            let Some(api_version) = &obj.api_version else {
                return Err(DawnStoreError::ApiVersionMissingInObject);
            };
            let Some(kind) = &obj.kind else {
                return Err(DawnStoreError::KindMissingInObject);
            };
            let object_id = ObjectId {
                kind: kind.clone(),
                api_version: api_version.clone(),
            };
            let validator = match schema_cache.get(&object_id) {
                Some(x) => x,
                None => {
                    drop(schema_cache);
                    let Some(schema) =
                        queries::get_object_schema(&self.pool, api_version, kind).await?
                    else {
                        return Err(DawnStoreError::NoSchemaForObjectFound {
                            api_version: api_version.clone(),
                            kind: kind.clone(),
                        });
                    };
                    let validator =
                        jsonschema::validator_for(&serde_json::from_str(&schema.json_schema)?)?;
                    {
                        self.schema_cache
                            .write()
                            .await
                            .insert(object_id.clone(), validator);
                    }
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
            let string_id = format!(
                "{}/{}/{}",
                if let Some(x) = &obj.namespace {
                    x.as_str()
                } else {
                    "default"
                },
                kind,
                obj.name,
            );
            // todo extract foreign keys from the spec and add to vector
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
                namespace: obj.namespace,
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
                namespace: x.namespace.unwrap_or_else(|| "default".to_string()),
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
