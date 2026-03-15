use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use uuid::Uuid;

// ── API response envelope ─────────────────────────────────────────────────────

/// Client-safe error variants returned inside [`DawnStoreResponse`].
/// Internal details (DB errors, stack traces) are never exposed.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DawnStoreApiError {
    UnknownResourceKind { kind: String },
    NamespaceRestriction { namespace: String },
    SchemaNotFound { api_version: String, kind: String },
    ValidationError { name: String, message: String },
    ForeignKeyNotFound { value: String },
    InvalidInput { message: String },
    Forbidden,
    InternalError,
}

/// Uniform API envelope. HTTP status is always 200 for application-level
/// errors; only transport/auth failures use non-200 status codes.
#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct DawnStoreResponse<T> {
    pub data: Option<T>,
    pub error: Option<DawnStoreApiError>,
}

impl<T> DawnStoreResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            data: Some(data),
            error: None,
        }
    }
    pub fn err(error: DawnStoreApiError) -> Self {
        Self {
            data: None,
            error: Some(error),
        }
    }
}

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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, JsonSchema)]
pub struct ReturnObject<T> {
    pub id: uuid::Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "is_none_or_empty")]
    pub annotations: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "is_none_or_empty")]
    pub labels: Option<BTreeMap<String, String>>,

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

/// A single permission scope used to restrict object queries to what the caller
/// is authorised to see.
///
/// - `namespace`: `None` = any namespace (from a global role binding).
/// - `kind`: `"*"` = any kind.
/// - `names`: `None` = all names permitted; `Some(vec)` = restrict to these names.
#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct AllowedScope {
    /// `None` means any namespace (global grant).
    pub namespace: Option<String>,
    /// `"*"` matches all kinds.
    pub kind: String,
    /// `None` means all names in this (namespace, kind) are permitted.
    pub names: Option<Vec<String>>,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Default, Clone)]
pub struct GetObjectsFilter {
    pub namespace: Option<String>,
    pub kind: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub fill_child_foreign_keys: bool,
    #[serde(default)]
    pub fill_parent_foreign_keys: bool,
    pub ids: Option<Vec<Uuid>>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct ListOfObjects {
    /// should always be list
    pub kind: String,
    pub object_kind: Option<String>,
    pub object_api_version: Option<String>,
    pub list: Vec<ObjectAny>,
}

#[derive(serde::Deserialize, serde::Serialize, Clone)]
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

#[derive(serde::Deserialize, serde::Serialize)]
pub struct ObjectInfo {
    pub namespace: String,
    pub id: Uuid,
    pub api_version: String,
    pub kind: String,
    pub name: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct ObjectInfos {
    pub infos: Vec<ObjectInfo>,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone, Default)]
pub struct GetObjectInfosFilter {
    pub namespace: Option<String>,
    pub kind: Option<String>,
    pub name: Option<String>,
    pub name_search_string: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

// ── Request / response ────────────────────────────────────────────────────────

#[derive(serde::Deserialize, serde::Serialize)]
pub struct IssueTokenRequest {
    /// Namespace of the target service account.
    pub namespace: String,
    /// Name of the target service account.
    pub service_account: String,
    /// A human-readable name for this token (also used as the dawnstore object name).
    pub token_name: String,
    /// Optional expiry. `None` defaults to 1 year from now.
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct IssueTokenResponse {
    pub token: String,
    pub token_id: Uuid,
    pub expires_at: DateTime<Utc>,
}
