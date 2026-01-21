use std::collections::{BTreeMap, HashMap};

use chrono::Utc;
use serde_json::Value;
use sqlx::{PgConnection, Pool, Postgres, migrate::MigrateError};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::{
    backends::postgres::data_models::{ForeignKeyConstraint, Object, ObjectSchema},
    error::DawnStoreError,
    models::{ForeignKey, ForeignKeyType},
};

use dawnstore_lib::*;

mod apply_impl;
mod data_models;
mod queries;

pub struct PostgresBackend {
    pool: Pool<Postgres>,
    foreign_key_cache: RwLock<HashMap<String, Vec<ForeignKeyConstraint>>>,
    schema_cache: RwLock<HashMap<String, jsonschema::Validator>>,
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
        data: serde_json::Value,
    ) -> Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError> {
        let input_objects = apply_impl::build_base_objects_from_raw_value(data)?;

        // validate if objects have all required fields and if the underlying schema is sound
        let mut string_ids = Vec::<String>::with_capacity(input_objects.len());
        let mut input_objects_with_string_id = Vec::<(String, ObjectAny)>::new();

        // let mut schema_cache = self.schema_cache.read().await;
        for obj in input_objects {
            let Some(api_version) = &obj.api_version else {
                return Err(DawnStoreError::ApiVersionMissingInObject);
            };
            let Some(kind) = &obj.kind else {
                return Err(DawnStoreError::KindMissingInObject);
            };
            let ns = obj.namespace.as_deref().unwrap_or("default");
            let object_id = format!("{api_version}/{kind}");
            let string_id = format!("{}/{}/{}", ns, kind, obj.name,);
            string_ids.push(string_id.clone());

            let mut con = self.pool.acquire().await?;
            apply_impl::validate_object_schema(
                &mut con,
                &self.schema_cache,
                &obj,
                api_version,
                kind,
                &object_id,
            )
            .await?;

            // check if the foreign keys are valid
            apply_impl::check_foreign_keys(
                &mut con,
                &self.foreign_key_cache,
                &string_ids,
                &obj,
                api_version,
                kind,
                ns,
                object_id,
            )
            .await?;

            // string_ids.push(string_id.clone());
            input_objects_with_string_id.push((string_id, obj));
        }

        let mut con = self.pool.begin().await?;
        let database_objects =
            apply_impl::maintain_objects(con.as_mut(), string_ids, input_objects_with_string_id)
                .await?;
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
                // todo set the owners
                owners: Some(Default::default()),
                spec: x.spec.0,
            })
            .collect())
    }
}
