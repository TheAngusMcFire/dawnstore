use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ObjectOwner {
    pub api_version: String,
    pub kind: String,
    pub name: String,
    pub id: uuid::Uuid,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Object<T> {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<uuid::Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owners: Option<Vec<ObjectOwner>>,
    pub spec: T,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ListObject<T> {
    pub namespace: String,
    pub api_version: String,
    pub kind: String,
    pub name: String,
    pub id: uuid::Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owners: Option<Vec<ObjectOwner>>,
    pub spec: T,
}

pub struct ListOfObjects {
    /// should always be list
    pub kind: String,
    pub object_kind: Option<String>,
    pub list: Vec<serde_json::Value>,
}
