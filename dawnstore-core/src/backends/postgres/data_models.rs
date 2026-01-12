use std::collections::BTreeMap;

use sqlx::types::{
    Json, Uuid,
    chrono::{DateTime, Utc},
};

pub struct ForeignKeyConstraint {
    pub id: uuid::Uuid,
    pub api_version: String,
    pub kind: String,
    pub key_path: String,
}

pub struct ObjectSchema {
    pub id: uuid::Uuid,
    pub api_version: String,
    pub kind: String,
    pub json_schema: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Object {
    pub id: Uuid,
    pub api_version: String,
    pub name: String,
    pub kind: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub namespace: Option<String>,
    pub annotations: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
    pub owners: Vec<Uuid>,
    pub spec: Json<serde_json::Value>,
}
