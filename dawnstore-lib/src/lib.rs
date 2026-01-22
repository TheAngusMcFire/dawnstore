use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;

#[derive(serde::Serialize, serde::Deserialize, Debug, JsonSchema)]
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
    pub created_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owners: Option<Vec<ObjectOwner>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub name: String,
    #[serde(flatten)]
    pub spec: T,
}
pub type ObjectAny = Object<serde_json::Value>;
pub type ReturnAny = ReturnObject<serde_json::Value>;
pub type Metadata = Object<Option<()>>;

#[derive(Debug, serde::Serialize, serde::Deserialize, JsonSchema)]
pub struct ReturnObject<T> {
    pub id: uuid::Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "is_none_or_empty")]
    pub annotations: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "is_none_or_empty")]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "vec_is_none_or_empty")]
    pub owners: Option<Vec<ObjectOwner>>,

    pub namespace: String,
    pub api_version: String,
    pub kind: String,
    pub name: String,

    #[serde(flatten)]
    pub spec: T,
}

fn is_none_or_empty(v: &Option<BTreeMap<String, String>>) -> bool {
    v.as_ref().is_none_or(|map| map.is_empty())
}
fn vec_is_none_or_empty<T>(v: &Option<Vec<T>>) -> bool {
    v.as_ref().is_none_or(|map| map.is_empty())
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct GetObjectsFilter {
    pub namespace: Option<String>,
    pub kind: Option<String>,
    pub name: Option<String>,
    pub fill_child_foreign_keys: bool,
    pub fill_parent_foreign_keys: bool,
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

#[derive(serde::Deserialize, serde::Serialize)]
pub struct DeleteObject {
    pub namespace: Option<String>,
    pub kind: String,
    pub name: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct ResourceDefinition {
    pub api_version: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub json_schema: String,
}

#[derive(Default, serde::Deserialize, serde::Serialize)]
pub struct GetResourceDefinitionFilter {}
