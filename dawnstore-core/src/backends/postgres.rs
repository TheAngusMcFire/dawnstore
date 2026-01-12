use std::collections::BTreeMap;

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
        let mut database_object = Vec::<Object>::new();
        for obj in &input_objects {
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
                todo!("throw validation error")
            }
        }

        todo!()
    }
}
