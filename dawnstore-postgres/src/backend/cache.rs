use std::{collections::HashMap, sync::Arc};

use sqlx::{PgConnection, PgPool};
use tokio::sync::RwLock;

use dawnstore_core::error::DawnStoreError;

use super::{data_models::ForeignKeyConstraint, queries};

pub struct CacheStore {
    schema_cache: RwLock<HashMap<String, jsonschema::Validator>>,
    foreign_key_cache: RwLock<HashMap<String, Arc<Vec<ForeignKeyConstraint>>>>,
}

impl Default for CacheStore {
    fn default() -> Self {
        Self {
            schema_cache: Default::default(),
            foreign_key_cache: Default::default(),
        }
    }
}

impl CacheStore {
    /// Populate both caches from the database. Call once at startup.
    pub async fn warm(&self, pool: &PgPool) -> Result<(), DawnStoreError> {
        let schemas = queries::get_all_object_schemas(pool).await?;
        let mut schema_cache = self.schema_cache.write().await;
        for schema in &schemas {
            let key = format!("{}/{}", schema.api_version, schema.kind);
            let validator =
                jsonschema::validator_for(&serde_json::from_str(&schema.json_schema)?)?;
            schema_cache.insert(key, validator);
        }
        drop(schema_cache);

        let all_fks = queries::get_all_foreign_key_constraints(pool).await?;
        let mut fk_cache = self.foreign_key_cache.write().await;
        let mut grouped: HashMap<String, Vec<ForeignKeyConstraint>> = HashMap::new();
        for fk in all_fks {
            let key = format!("{}/{}", fk.api_version, fk.kind);
            grouped.entry(key).or_default().push(fk);
        }
        for (key, vec) in grouped {
            fk_cache.insert(key, Arc::new(vec));
        }

        Ok(())
    }

    /// Insert a compiled schema validator into the cache.
    pub async fn insert_schema(&self, api_version: &str, kind: &str, validator: jsonschema::Validator) {
        let key = format!("{api_version}/{kind}");
        self.schema_cache.write().await.insert(key, validator);
    }

    /// Insert foreign key constraints into the cache.
    pub async fn insert_foreign_keys(
        &self,
        api_version: &str,
        kind: &str,
        constraints: Vec<ForeignKeyConstraint>,
    ) {
        let key = format!("{api_version}/{kind}");
        self.foreign_key_cache.write().await.insert(key, Arc::new(constraints));
    }

    /// Validate `spec` against the registered JSON schema for `api_version/kind`.
    /// On a cache miss the schema is loaded from the database and cached.
    pub async fn validate_schema(
        &self,
        pool: &mut PgConnection,
        api_version: &str,
        kind: &str,
        name: &str,
        spec: &serde_json::Value,
    ) -> Result<(), DawnStoreError> {
        let key = format!("{api_version}/{kind}");

        {
            let cache = self.schema_cache.read().await;
            if let Some(validator) = cache.get(&key) {
                return Self::run_validation(validator, spec, api_version, kind, name);
            }
        }

        // Cache miss: load from DB, compile, and cache.
        let schema = queries::get_object_schema(pool, api_version, kind)
            .await?
            .ok_or_else(|| DawnStoreError::NoSchemaForObjectFound {
                api_version: api_version.to_owned(),
                kind: kind.to_owned(),
            })?;
        let validator = jsonschema::validator_for(&serde_json::from_str(&schema.json_schema)?)?;
        let result = Self::run_validation(&validator, spec, api_version, kind, name);
        self.schema_cache.write().await.insert(key, validator);
        result
    }

    /// Return the foreign key constraints for `api_version/kind`.
    /// On a cache miss they are loaded from the database and cached.
    pub async fn get_foreign_keys(
        &self,
        pool: &mut PgConnection,
        api_version: &str,
        kind: &str,
    ) -> Result<Arc<Vec<ForeignKeyConstraint>>, DawnStoreError> {
        let key = format!("{api_version}/{kind}");

        {
            let cache = self.foreign_key_cache.read().await;
            if let Some(fks) = cache.get(&key) {
                return Ok(Arc::clone(fks));
            }
        }

        // Cache miss: load from DB and cache.
        let constraints = Arc::new(queries::get_foreign_key_constraints(pool, api_version, kind).await?);
        self.foreign_key_cache
            .write()
            .await
            .insert(key, Arc::clone(&constraints));
        Ok(constraints)
    }

    fn run_validation(
        validator: &jsonschema::Validator,
        spec: &serde_json::Value,
        api_version: &str,
        kind: &str,
        name: &str,
    ) -> Result<(), DawnStoreError> {
        if let Err(e) = validator.validate(spec) {
            return Err(DawnStoreError::ObjectValidationError {
                api_version: api_version.to_owned(),
                kind: kind.to_owned(),
                name: name.to_owned(),
                validation_error: e.to_owned(),
            });
        }
        Ok(())
    }
}
