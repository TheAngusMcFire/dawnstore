use std::collections::{BTreeMap, HashMap};

use chrono::Utc;
use serde_json::Value;
use sqlx::PgConnection;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::{
    backends::postgres::{
        data_models::{ForeignKeyConstraint, Object},
        queries,
    },
    error::DawnStoreError,
    models::ForeignKeyType,
};

use dawnstore_lib::*;

pub fn build_base_objects_from_raw_value(
    mut data: Value,
) -> Result<Vec<dawnstore_lib::Object<Value>>, DawnStoreError> {
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
                    if let Some(Value::String(object_api_version)) = x.get("object_api_version") {
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
    Ok(input_objects)
}

pub async fn validate_object_schema(
    pool: &mut PgConnection,
    sc: &RwLock<HashMap<String, jsonschema::Validator>>,
    obj: &dawnstore_lib::Object<Value>,
    api_version: &str,
    kind: &str,
    object_id: &String,
) -> Result<(), DawnStoreError> {
    let mut schema_cache = sc.read().await;
    let validator = match schema_cache.get(object_id) {
        Some(x) => x,
        None => {
            drop(schema_cache);
            let Some(schema) = queries::get_object_schema(pool, api_version, kind).await? else {
                return Err(DawnStoreError::NoSchemaForObjectFound {
                    api_version: api_version.to_owned(),
                    kind: kind.to_owned(),
                });
            };
            let validator = jsonschema::validator_for(&serde_json::from_str(&schema.json_schema)?)?;
            sc.write().await.insert(object_id.clone(), validator);
            schema_cache = sc.read().await;
            schema_cache
                .get(object_id)
                .expect("we just added this thing")
        }
    };

    if let Err(e) = validator.validate(&obj.spec) {
        return Err(DawnStoreError::ObjectValidationError {
            api_version: api_version.to_owned(),
            kind: kind.to_owned(),
            name: obj.name.clone(),
            validation_error: e.to_owned(),
        });
    };

    Ok(())
}

pub async fn check_foreign_keys(
    pool: &mut PgConnection,
    fkc: &RwLock<HashMap<String, Vec<ForeignKeyConstraint>>>,
    obj: &dawnstore_lib::Object<Value>,
    api_version: &str,
    kind: &str,
    ns: &str,
    type_id: String,
) -> Result<Vec<(Vec<String>, Uuid)>, DawnStoreError> {
    let mut foreign_key_cache = fkc.read().await;
    let foreign_keys = match foreign_key_cache.get(&type_id) {
        Some(x) => x,
        None => {
            drop(foreign_key_cache);
            let costraints = queries::get_foreign_key_constraints(pool, api_version, kind).await?;
            fkc.write().await.insert(type_id.clone(), costraints);
            foreign_key_cache = fkc.read().await;
            foreign_key_cache
                .get(&type_id)
                .expect("we just added the constraints")
        }
    };

    let mut fk_string_ids: Vec<(Vec<String>, Uuid)> = Default::default();
    'outer: for key in foreign_keys {
        let path_segments = key.key_path.split(".");
        let mut key_position = None::<&Value>;
        for seg in path_segments {
            let k = match key_position {
                Some(x) => x.get(seg),
                None => obj.spec.get(seg),
            };
            key_position = match k {
                Some(x) => Some(x),
                None if key.r#type == ForeignKeyType::OneOptional => continue 'outer,
                None if key.r#type == ForeignKeyType::NoneOrMany => continue 'outer,
                None => {
                    return Err(DawnStoreError::ObjectValidationMissingForeignKeyEntry {
                        api_version: api_version.to_owned(),
                        kind: kind.to_owned(),
                        name: obj.name.clone(),
                        foreign_key_path: key.key_path.clone(),
                        foreign_key_type: key.r#type.clone(),
                    });
                }
            };
        }

        let Some(key_position) = key_position else {
            return Err(DawnStoreError::ObjectValidationMissingForeignKeyEntry {
                api_version: api_version.to_owned(),
                kind: kind.to_owned(),
                name: obj.name.clone(),
                foreign_key_path: key.key_path.clone(),
                foreign_key_type: key.r#type.clone(),
            });
        };

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
                    api_version: api_version.to_owned(),
                    kind: kind.to_owned(),
                    name: obj.name.clone(),
                    foreign_key_path: key.key_path.clone(),
                    foreign_key_type: key.r#type.clone(),
                });
            }
        };

        let mut fks = Vec::with_capacity(foreign_key_values.len());
        for fk_val in foreign_key_values {
            let comps = fk_val.split("/").collect::<Vec<_>>();
            let (ns, fk_kind, fk_name) = match comps.as_slice() {
                [ns, kind, name] => (*ns, *kind, *name),
                // assume same ns as the current object
                [kind, name] => (ns, *kind, *name),
                // assume same ns and kind as the current object
                [name] => (ns, kind, *name),
                _ => {
                    return Err(DawnStoreError::ObjectValidationWrongForeignKeyEntryFormat {
                        api_version: api_version.to_owned(),
                        kind: kind.to_owned(),
                        name: obj.name.clone(),
                        foreign_key_path: key.key_path.clone(),
                        foreign_key_type: key.r#type.clone(),
                        value: fk_val.clone(),
                    });
                }
            };

            if let Some(k) = &key.foreign_key_kind
                && k.as_str() != fk_kind
            {
                return Err(DawnStoreError::ObjectValidationWrongForeignKeyEntryKind {
                    api_version: api_version.to_owned(),
                    kind: kind.to_owned(),
                    name: obj.name.clone(),
                    foreign_key_path: key.key_path.clone(),
                    foreign_key_type: key.r#type.clone(),
                    value: fk_val.clone(),
                });
            }

            fks.push(format!("{ns}/{fk_kind}/{fk_name}"));
        }

        fk_string_ids.push((fks, key.id));
    }

    Ok(fk_string_ids)
}

pub async fn maintain_objects(
    con: &mut PgConnection,
    string_ids: Vec<String>,
    input_objects_with_string_id: Vec<(String, dawnstore_lib::Object<Value>)>,
) -> Result<Vec<Object>, DawnStoreError> {
    let mut database_objects = Vec::<Object>::with_capacity(input_objects_with_string_id.len());
    let object_infos = queries::get_object_infos(con, string_ids.as_slice()).await?;
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
    queries::insert_or_update_multiple_objects(con, &database_objects).await?;
    Ok(database_objects)
}
