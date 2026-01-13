use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use jsonschema::ValidationError;
use serde_json::Value;
use sqlx::{Pool, Postgres, migrate::MigrateError};
use tokio::sync::RwLock;

use crate::{
    backends::postgres::data_models::Object,
    error::DawnStoreError,
    models::{ObjectAny, ObjectId, ReturnObject},
};

mod data_models;
mod queries;

pub struct PostgresBackend {
    pub pool: Pool<Postgres>,
    pub foraign_key_cache: RwLock<BTreeMap<(String, String), Vec<String>>>,
    pub schema_cache: RwLock<BTreeMap<ObjectId, jsonschema::Validator>>,
}

impl PostgresBackend {
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

        let mut database_objects_to_create =
            Vec::<Object>::with_capacity(input_objects_with_string_id.len());
        let mut database_objects_to_update =
            Vec::<Object>::with_capacity(input_objects_with_string_id.len());

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
            if oi.is_some() {
                database_objects_to_update.push(new_obj);
            } else {
                database_objects_to_create.push(new_obj);
            }
        }

        todo!()
    }
}
