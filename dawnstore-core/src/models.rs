use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

#[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct EmptyObject {}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct ObjectId {
    pub kind: String,
    pub api_version: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct ObjectOwner {
    pub api_version: String,
    pub kind: String,
    pub name: String,
    pub id: uuid::Uuid,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct Object<T> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<uuid::Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owners: Option<Vec<ObjectOwner>>,
    pub spec: T,
}
pub type ObjectAny = Object<serde_json::Value>;
pub type ReturnAny = ReturnObject<serde_json::Value>;
pub type Metadata = Object<Option<()>>;

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ReturnObject<T> {
    pub id: uuid::Uuid,
    pub namespace: String,
    pub api_version: String,
    pub kind: String,
    pub name: String,
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

#[derive(serde::Deserialize, Debug)]
pub struct ListObjectsFilter {
    pub namespace: Option<String>,
    pub kind: Option<String>,
    pub name: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

pub struct ListOfObjects {
    /// should always be list
    pub kind: String,
    pub object_kind: Option<String>,
    pub object_api_version: Option<String>,
    pub list: Vec<ObjectAny>,
}

#[derive(serde::Deserialize)]
pub struct DeleteObject {
    pub namespace: Option<String>,
    pub kind: String,
    pub name: String,
}
