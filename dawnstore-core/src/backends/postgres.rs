use std::collections::BTreeMap;

use serde_json::Value;
use sqlx::{Pool, Postgres, migrate::MigrateError};
use tokio::sync::RwLock;

use crate::{
    backends::postgres::data_models::Object,
    error::DawnStoreError,
    models::{ObjectAny, ReturnObject},
};

mod data_models;
mod queries;

pub struct PostgresBackend {
    pub pool: Pool<Postgres>,
    pub foraign_key_cache: RwLock<BTreeMap<(String, String), Vec<String>>>,
    pub schema_cache: RwLock<BTreeMap<(String, String), jsonschema::Validator>>,
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
        let mut data_base_object = Vec::<Object>::new();
        for obj in &input_objects {
            // schema_cache
        }

        todo!()
    }
}
