use std::collections::BTreeMap;

use sqlx::{
    prelude::FromRow,
    types::{
        Json, Uuid,
        chrono::{DateTime, Utc},
    },
};
#[derive(FromRow)]
pub struct ForeignKeyConstraint {
    pub id: uuid::Uuid,
    pub api_version: String,
    pub kind: String,
    pub key_path: String,
}

#[derive(FromRow)]
pub struct ObjectSchema {
    pub id: uuid::Uuid,
    pub api_version: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub json_schema: String,
}

#[derive(FromRow, serde::Serialize, serde::Deserialize, Debug)]
pub struct Object {
    pub id: Uuid,
    pub string_id: String,
    pub api_version: String,
    pub name: String,
    pub kind: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub namespace: Option<String>,
    pub annotations: Json<BTreeMap<String, String>>,
    pub labels: Json<BTreeMap<String, String>>,
    pub owners: Vec<Uuid>,
    pub spec: Json<serde_json::Value>,
}

#[derive(FromRow, serde::Serialize, serde::Deserialize)]
pub struct ObjectInfo {
    pub id: Uuid,
    pub string_id: String,
    pub created_at: DateTime<Utc>,
}
